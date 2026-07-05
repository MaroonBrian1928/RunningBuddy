use crate::{strava::WebhookEvent, AppState};
use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use reqwest::StatusCode;
use serde_json::{json, Value};
use sqlx::FromRow;
use std::time::Duration;
use tokio::task::JoinHandle;

const STRAVA_API_BASE: &str = "https://www.strava.com/api/v3";
const STREAM_KEYS: &str = "time,distance,heartrate,cadence,velocity_smooth,watts,altitude";
const MAX_ATTEMPTS: i64 = 5;

pub fn spawn_worker(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Err(err) = process_next_job(&state).await {
                tracing::warn!(error = %err, "sync worker tick failed");
            }
        }
    })
}

pub async fn process_next_job(state: &AppState) -> anyhow::Result<bool> {
    let Some(job) = claim_next_job(state).await? else {
        return Ok(false);
    };

    let result = process_job(state, &job).await;
    match result {
        Ok(()) => complete_job(state, job.id).await?,
        Err(err) => retry_or_fail_job(state, &job, err).await?,
    }

    Ok(true)
}

async fn process_job(state: &AppState, job: &SyncJob) -> anyhow::Result<()> {
    match job.job_type.as_str() {
        "manual_sync" => {
            let payload: Value =
                serde_json::from_str(&job.payload_json).unwrap_or_else(|_| json!({}));
            let backfill_days = payload
                .get("backfill_days")
                .and_then(Value::as_i64)
                .unwrap_or(state.config.strava_backfill_days);
            sync_all_athletes(state, backfill_days).await
        }
        "webhook_event" => {
            let event: WebhookEvent = serde_json::from_str(&job.payload_json)?;
            process_webhook_event(state, event).await
        }
        other => bail!("unknown sync job type: {other}"),
    }
}

async fn sync_all_athletes(state: &AppState, backfill_days: i64) -> anyhow::Result<()> {
    let tokens = load_token_rows(state).await?;
    for mut token in tokens {
        refresh_token_if_needed(state, &mut token).await?;
        sync_athlete_profile(state, &token).await?;
        sync_athlete_activities(state, &token, backfill_days).await?;
    }
    Ok(())
}

async fn process_webhook_event(state: &AppState, event: WebhookEvent) -> anyhow::Result<()> {
    match (event.object_type.as_str(), event.aspect_type.as_str()) {
        ("activity", "delete") => mark_activity_deleted(state, event.object_id).await,
        ("activity", "create" | "update") => {
            sync_activity_for_owner(state, event.owner_id, event.object_id).await
        }
        ("athlete", "update") if event.updates.get("authorized") == Some(&Value::Bool(false)) => {
            disconnect_athlete(state, event.owner_id).await
        }
        _ => Ok(()),
    }
}

async fn sync_athlete_activities(
    state: &AppState,
    token: &TokenRow,
    backfill_days: i64,
) -> anyhow::Result<()> {
    let after = Utc::now()
        .checked_sub_signed(chrono::Duration::days(backfill_days))
        .unwrap_or_else(Utc::now)
        .timestamp();

    for page in 1.. {
        let url =
            format!("{STRAVA_API_BASE}/athlete/activities?after={after}&per_page=100&page={page}");
        let activities = strava_get_json(state, &token.access_token, &url).await?;
        let activities = activities
            .as_array()
            .ok_or_else(|| anyhow!("Strava activities response was not an array"))?;
        if activities.is_empty() {
            break;
        }

        for activity in activities {
            let Some(strava_activity_id) = activity.get("id").and_then(Value::as_i64) else {
                continue;
            };
            if activity_summary_is_current(state, strava_activity_id, activity).await? {
                continue;
            }
            sync_activity_with_token(state, token, strava_activity_id, false).await?;
        }
    }

    Ok(())
}

async fn sync_activity_for_owner(
    state: &AppState,
    strava_athlete_id: i64,
    strava_activity_id: i64,
) -> anyhow::Result<()> {
    let mut token = load_token_for_strava_athlete(state, strava_athlete_id)
        .await?
        .ok_or_else(|| anyhow!("no Strava token for athlete {strava_athlete_id}"))?;
    refresh_token_if_needed(state, &mut token).await?;
    sync_activity_with_token(state, &token, strava_activity_id, true).await
}

async fn sync_activity_with_token(
    state: &AppState,
    token: &TokenRow,
    strava_activity_id: i64,
    refresh_streams: bool,
) -> anyhow::Result<()> {
    let detail_url =
        format!("{STRAVA_API_BASE}/activities/{strava_activity_id}?include_all_efforts=false");
    let detail = match strava_get_json(state, &token.access_token, &detail_url).await {
        Ok(detail) => detail,
        Err(err) if is_private_or_missing(&err) => {
            mark_activity_private_unavailable(state, token.athlete_id, strava_activity_id).await?;
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    upsert_activity(state, token.athlete_id, &detail).await?;
    if refresh_streams || !activity_streams_exist(state, strava_activity_id).await? {
        if let Some(streams) =
            fetch_activity_streams(state, &token.access_token, strava_activity_id).await?
        {
            upsert_streams(state, strava_activity_id, &streams).await?;
        }
    }
    Ok(())
}

async fn activity_summary_is_current(
    state: &AppState,
    strava_activity_id: i64,
    activity: &Value,
) -> anyhow::Result<bool> {
    let Some(stored) = load_stored_activity_summary(state, strava_activity_id).await? else {
        return Ok(false);
    };

    Ok(stored_activity_matches_list_summary(&stored, activity))
}

async fn sync_athlete_profile(state: &AppState, token: &TokenRow) -> anyhow::Result<()> {
    let athlete = strava_get_json(
        state,
        &token.access_token,
        &format!("{STRAVA_API_BASE}/athlete"),
    )
    .await?;

    sqlx::query(
        r#"
        UPDATE athletes
        SET username = ?,
            firstname = ?,
            lastname = ?,
            profile_url = ?,
            raw_profile_json = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(athlete.get("username").and_then(Value::as_str))
    .bind(athlete.get("firstname").and_then(Value::as_str))
    .bind(athlete.get("lastname").and_then(Value::as_str))
    .bind(athlete.get("profile").and_then(Value::as_str))
    .bind(athlete.to_string())
    .bind(token.athlete_id)
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn load_stored_activity_summary(
    state: &AppState,
    strava_activity_id: i64,
) -> anyhow::Result<Option<StoredActivitySummary>> {
    Ok(sqlx::query_as::<_, StoredActivitySummary>(
        r#"
        SELECT name, sport_type, start_date, elapsed_time_seconds, moving_time_seconds,
               distance_meters, total_elevation_gain, average_heartrate, average_speed,
               max_speed, average_cadence, average_watts, kilojoules, suffer_score,
               visibility, deleted_at, private_unavailable
        FROM activities
        WHERE strava_activity_id = ?
        "#,
    )
    .bind(strava_activity_id)
    .fetch_optional(&state.db)
    .await?)
}

fn stored_activity_matches_list_summary(stored: &StoredActivitySummary, activity: &Value) -> bool {
    if stored.deleted_at.is_some() || stored.private_unavailable != 0 {
        return false;
    }

    string_matches(&stored.name, activity.get("name").and_then(Value::as_str))
        && optional_string_matches(
            &stored.sport_type,
            activity
                .get("sport_type")
                .or_else(|| activity.get("type"))
                .and_then(Value::as_str),
        )
        && optional_string_matches(
            &stored.start_date,
            activity.get("start_date").and_then(Value::as_str),
        )
        && i64_matches(
            stored.elapsed_time_seconds,
            activity.get("elapsed_time").and_then(Value::as_i64),
        )
        && i64_matches(
            stored.moving_time_seconds,
            activity.get("moving_time").and_then(Value::as_i64),
        )
        && f64_matches(
            stored.distance_meters,
            activity.get("distance").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.total_elevation_gain,
            activity.get("total_elevation_gain").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.average_heartrate,
            activity.get("average_heartrate").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.average_speed,
            activity.get("average_speed").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.max_speed,
            activity.get("max_speed").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.average_cadence,
            activity.get("average_cadence").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.average_watts,
            activity.get("average_watts").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.kilojoules,
            activity.get("kilojoules").and_then(Value::as_f64),
        )
        && f64_matches(
            stored.suffer_score,
            activity.get("suffer_score").and_then(Value::as_f64),
        )
        && optional_string_matches(
            &stored.visibility,
            activity.get("visibility").and_then(Value::as_str),
        )
}

fn string_matches(stored: &str, listed: Option<&str>) -> bool {
    listed.is_none_or(|value| stored == value)
}

fn optional_string_matches(stored: &Option<String>, listed: Option<&str>) -> bool {
    listed.is_none_or(|value| stored.as_deref() == Some(value))
}

fn i64_matches(stored: Option<i64>, listed: Option<i64>) -> bool {
    listed.is_none_or(|value| stored == Some(value))
}

fn f64_matches(stored: Option<f64>, listed: Option<f64>) -> bool {
    listed.is_none_or(|value| {
        stored
            .map(|stored_value| (stored_value - value).abs() < 0.001)
            .unwrap_or(false)
    })
}

async fn fetch_activity_streams(
    state: &AppState,
    access_token: &str,
    strava_activity_id: i64,
) -> anyhow::Result<Option<Value>> {
    let url = format!(
        "{STRAVA_API_BASE}/activities/{strava_activity_id}/streams?keys={STREAM_KEYS}&key_by_type=true"
    );
    let response = state.http.get(url).bearer_auth(access_token).send().await?;
    check_rate_limit(response.headers())?;

    match response.status() {
        StatusCode::OK => Ok(Some(response.json::<Value>().await?)),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(None),
        StatusCode::UNAUTHORIZED => bail!("Strava API unauthorized: 401 Unauthorized"),
        StatusCode::TOO_MANY_REQUESTS => bail!("Strava rate limit exceeded"),
        status if status.is_server_error() => bail!("transient Strava stream error: {status}"),
        status => bail!("Strava stream request failed: {status}"),
    }
}

async fn strava_get_json(state: &AppState, access_token: &str, url: &str) -> anyhow::Result<Value> {
    let response = state.http.get(url).bearer_auth(access_token).send().await?;
    check_rate_limit(response.headers())?;
    let status = response.status();

    if status == StatusCode::TOO_MANY_REQUESTS {
        bail!("Strava rate limit exceeded");
    }
    if status == StatusCode::UNAUTHORIZED {
        bail!("Strava API unauthorized: {status}");
    }
    if status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN {
        bail!("Strava private or unavailable resource: {status}");
    }
    if status.is_server_error() {
        bail!("transient Strava API error: {status}");
    }
    if !status.is_success() {
        bail!("Strava API request failed: {status}");
    }

    Ok(response.json::<Value>().await?)
}

fn check_rate_limit(headers: &reqwest::header::HeaderMap) -> anyhow::Result<()> {
    let Some(limit) = headers
        .get("x-ratelimit-limit")
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };
    let Some(usage) = headers
        .get("x-ratelimit-usage")
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };

    if rate_limit_exhausted(limit, usage) {
        bail!("Strava rate limit exhausted: usage={usage} limit={limit}");
    }

    Ok(())
}

fn rate_limit_exhausted(limit: &str, usage: &str) -> bool {
    let parse_pair = |value: &str| -> Option<(i64, i64)> {
        let (short, long) = value.split_once(',')?;
        Some((short.trim().parse().ok()?, long.trim().parse().ok()?))
    };
    let Some((short_limit, long_limit)) = parse_pair(limit) else {
        return false;
    };
    let Some((short_usage, long_usage)) = parse_pair(usage) else {
        return false;
    };
    short_usage >= short_limit || long_usage >= long_limit
}

async fn refresh_token_if_needed(state: &AppState, token: &mut TokenRow) -> anyhow::Result<()> {
    if token.expires_at > Utc::now().timestamp() + 60 {
        return Ok(());
    }

    let client_id = state
        .config
        .strava_client_id
        .as_ref()
        .ok_or_else(|| anyhow!("STRAVA_CLIENT_ID is not configured"))?;
    let client_secret = state
        .config
        .strava_client_secret
        .as_ref()
        .ok_or_else(|| anyhow!("STRAVA_CLIENT_SECRET is not configured"))?;

    let response = state
        .http
        .post("https://www.strava.com/oauth/token")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", token.refresh_token.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;

    token.access_token = response
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    token.refresh_token = response
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or(&token.refresh_token)
        .to_string();
    token.expires_at = response
        .get("expires_at")
        .and_then(Value::as_i64)
        .unwrap_or(token.expires_at);

    sqlx::query(
        r#"
        UPDATE strava_tokens
        SET access_token = ?, refresh_token = ?, expires_at = ?, updated_at = CURRENT_TIMESTAMP
        WHERE athlete_id = ?
        "#,
    )
    .bind(&token.access_token)
    .bind(&token.refresh_token)
    .bind(token.expires_at)
    .bind(token.athlete_id)
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn upsert_activity(
    state: &AppState,
    athlete_id: i64,
    activity: &Value,
) -> anyhow::Result<()> {
    let strava_activity_id = required_i64(activity, "id")?;
    sqlx::query(
        r#"
        INSERT INTO activities
            (strava_activity_id, athlete_id, name, sport_type, start_date, start_date_local,
             elapsed_time_seconds, moving_time_seconds, distance_meters,
             total_elevation_gain, average_heartrate, max_heartrate,
             average_speed, max_speed, average_cadence, average_watts,
             kilojoules, suffer_score, visibility, deleted_at, private_unavailable,
             raw_activity_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0, ?)
        ON CONFLICT(strava_activity_id) DO UPDATE SET
            athlete_id = excluded.athlete_id,
            name = excluded.name,
            sport_type = excluded.sport_type,
            start_date = excluded.start_date,
            start_date_local = excluded.start_date_local,
            elapsed_time_seconds = excluded.elapsed_time_seconds,
            moving_time_seconds = excluded.moving_time_seconds,
            distance_meters = excluded.distance_meters,
            total_elevation_gain = excluded.total_elevation_gain,
            average_heartrate = excluded.average_heartrate,
            max_heartrate = excluded.max_heartrate,
            average_speed = excluded.average_speed,
            max_speed = excluded.max_speed,
            average_cadence = excluded.average_cadence,
            average_watts = excluded.average_watts,
            kilojoules = excluded.kilojoules,
            suffer_score = excluded.suffer_score,
            visibility = excluded.visibility,
            deleted_at = NULL,
            private_unavailable = 0,
            raw_activity_json = excluded.raw_activity_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(strava_activity_id)
    .bind(athlete_id)
    .bind(
        activity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled activity"),
    )
    .bind(
        activity
            .get("sport_type")
            .or_else(|| activity.get("type"))
            .and_then(Value::as_str),
    )
    .bind(activity.get("start_date").and_then(Value::as_str))
    .bind(activity.get("start_date_local").and_then(Value::as_str))
    .bind(activity.get("elapsed_time").and_then(Value::as_i64))
    .bind(activity.get("moving_time").and_then(Value::as_i64))
    .bind(activity.get("distance").and_then(Value::as_f64))
    .bind(activity.get("total_elevation_gain").and_then(Value::as_f64))
    .bind(activity.get("average_heartrate").and_then(Value::as_f64))
    .bind(activity.get("max_heartrate").and_then(Value::as_f64))
    .bind(activity.get("average_speed").and_then(Value::as_f64))
    .bind(activity.get("max_speed").and_then(Value::as_f64))
    .bind(activity.get("average_cadence").and_then(Value::as_f64))
    .bind(activity.get("average_watts").and_then(Value::as_f64))
    .bind(activity.get("kilojoules").and_then(Value::as_f64))
    .bind(activity.get("suffer_score").and_then(Value::as_f64))
    .bind(activity.get("visibility").and_then(Value::as_str))
    .bind(activity.to_string())
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn upsert_streams(
    state: &AppState,
    strava_activity_id: i64,
    streams: &Value,
) -> anyhow::Result<()> {
    let activity_id: (i64,) =
        sqlx::query_as("SELECT id FROM activities WHERE strava_activity_id = ?")
            .bind(strava_activity_id)
            .fetch_one(&state.db)
            .await
            .context("activity must exist before streams are stored")?;

    sqlx::query(
        r#"
        INSERT INTO activity_streams
            (activity_id, time_json, distance_json, heartrate_json, cadence_json,
             velocity_smooth_json, watts_json, altitude_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(activity_id) DO UPDATE SET
            time_json = excluded.time_json,
            distance_json = excluded.distance_json,
            heartrate_json = excluded.heartrate_json,
            cadence_json = excluded.cadence_json,
            velocity_smooth_json = excluded.velocity_smooth_json,
            watts_json = excluded.watts_json,
            altitude_json = excluded.altitude_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(activity_id.0)
    .bind(stream_data(streams, "time"))
    .bind(stream_data(streams, "distance"))
    .bind(stream_data(streams, "heartrate"))
    .bind(stream_data(streams, "cadence"))
    .bind(stream_data(streams, "velocity_smooth"))
    .bind(stream_data(streams, "watts"))
    .bind(stream_data(streams, "altitude"))
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn activity_streams_exist(state: &AppState, strava_activity_id: i64) -> anyhow::Result<bool> {
    let exists: (i64,) = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM activity_streams s
            JOIN activities a ON a.id = s.activity_id
            WHERE a.strava_activity_id = ?
        )
        "#,
    )
    .bind(strava_activity_id)
    .fetch_one(&state.db)
    .await?;
    Ok(exists.0 != 0)
}

fn stream_data(streams: &Value, key: &str) -> Option<String> {
    streams
        .get(key)
        .and_then(|stream| stream.get("data"))
        .map(Value::to_string)
}

async fn mark_activity_deleted(state: &AppState, strava_activity_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE activities SET deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE strava_activity_id = ?",
    )
    .bind(strava_activity_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn mark_activity_private_unavailable(
    state: &AppState,
    athlete_id: i64,
    strava_activity_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO activities
            (strava_activity_id, athlete_id, name, private_unavailable, raw_activity_json)
        VALUES (?, ?, ?, 1, ?)
        ON CONFLICT(strava_activity_id) DO UPDATE SET
            private_unavailable = 1,
            raw_activity_json = excluded.raw_activity_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(strava_activity_id)
    .bind(athlete_id)
    .bind("Private or unavailable activity")
    .bind(json!({ "id": strava_activity_id, "private_unavailable": true }).to_string())
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn disconnect_athlete(state: &AppState, strava_athlete_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        DELETE FROM strava_tokens
        WHERE athlete_id IN (SELECT id FROM athletes WHERE strava_athlete_id = ?)
        "#,
    )
    .bind(strava_athlete_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn claim_next_job(state: &AppState) -> anyhow::Result<Option<SyncJob>> {
    let job = sqlx::query_as::<_, SyncJob>(
        r#"
        SELECT id, job_type, payload_json, attempts
        FROM sync_jobs
        WHERE status = 'queued' AND run_after <= CURRENT_TIMESTAMP
        ORDER BY id
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    if let Some(job) = &job {
        sqlx::query(
            "UPDATE sync_jobs SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(job.id)
        .execute(&state.db)
        .await?;
    }

    Ok(job)
}

async fn complete_job(state: &AppState, job_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sync_jobs SET status = 'completed', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(job_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn retry_or_fail_job(
    state: &AppState,
    job: &SyncJob,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    let attempts = job.attempts + 1;
    let error = err.to_string();
    let status = if attempts >= MAX_ATTEMPTS || is_non_retryable_sync_error(&error) {
        "failed"
    } else {
        "queued"
    };
    let delay = retry_delay_seconds(attempts);
    let modifier = format!("+{delay} seconds");

    sqlx::query(
        r#"
        UPDATE sync_jobs
        SET status = ?,
            attempts = ?,
            run_after = datetime('now', ?),
            last_error = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(status)
    .bind(attempts)
    .bind(modifier)
    .bind(error)
    .bind(job.id)
    .execute(&state.db)
    .await?;
    Ok(())
}

fn is_non_retryable_sync_error(error: &str) -> bool {
    error.contains("unauthorized") || error.contains("401")
}

fn retry_delay_seconds(attempts: i64) -> i64 {
    let capped = attempts.clamp(1, 6);
    (30_i64 * 2_i64.pow((capped - 1) as u32)).min(3600)
}

async fn load_token_rows(state: &AppState) -> anyhow::Result<Vec<TokenRow>> {
    Ok(sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT t.athlete_id, t.access_token, t.refresh_token, t.expires_at
        FROM strava_tokens t
        JOIN athletes a ON a.id = t.athlete_id
        "#,
    )
    .fetch_all(&state.db)
    .await?)
}

async fn load_token_for_strava_athlete(
    state: &AppState,
    strava_athlete_id: i64,
) -> anyhow::Result<Option<TokenRow>> {
    Ok(sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT t.athlete_id, t.access_token, t.refresh_token, t.expires_at
        FROM strava_tokens t
        JOIN athletes a ON a.id = t.athlete_id
        WHERE a.strava_athlete_id = ?
        "#,
    )
    .bind(strava_athlete_id)
    .fetch_optional(&state.db)
    .await?)
}

fn required_i64(value: &Value, key: &str) -> anyhow::Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing required integer field {key}"))
}

fn is_private_or_missing(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("private or unavailable")
}

#[derive(Debug, FromRow)]
struct SyncJob {
    id: i64,
    job_type: String,
    payload_json: String,
    attempts: i64,
}

#[derive(Debug, FromRow)]
struct TokenRow {
    athlete_id: i64,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

#[derive(Debug, FromRow)]
struct StoredActivitySummary {
    name: String,
    sport_type: Option<String>,
    start_date: Option<String>,
    elapsed_time_seconds: Option<i64>,
    moving_time_seconds: Option<i64>,
    distance_meters: Option<f64>,
    total_elevation_gain: Option<f64>,
    average_heartrate: Option<f64>,
    average_speed: Option<f64>,
    max_speed: Option<f64>,
    average_cadence: Option<f64>,
    average_watts: Option<f64>,
    kilojoules: Option<f64>,
    suffer_score: Option<f64>,
    visibility: Option<String>,
    deleted_at: Option<String>,
    private_unavailable: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_exhausted_rate_limit_header_pairs() {
        assert!(rate_limit_exhausted("100,1000", "100,12"));
        assert!(rate_limit_exhausted("100,1000", "40,1000"));
        assert!(!rate_limit_exhausted("100,1000", "40,12"));
        assert!(!rate_limit_exhausted("bad", "40,12"));
    }

    #[test]
    fn retry_delay_backs_off_and_caps() {
        assert_eq!(retry_delay_seconds(1), 30);
        assert_eq!(retry_delay_seconds(2), 60);
        assert_eq!(retry_delay_seconds(3), 120);
        assert_eq!(retry_delay_seconds(99), 960);
    }

    #[test]
    fn treats_unauthorized_sync_errors_as_non_retryable() {
        assert!(is_non_retryable_sync_error(
            "Strava API unauthorized: 401 Unauthorized"
        ));
        assert!(!is_non_retryable_sync_error(
            "transient Strava API error: 503"
        ));
    }

    #[test]
    fn extracts_stream_data_arrays() {
        let streams = json!({
            "time": { "data": [0, 1, 2] },
            "distance": { "data": [0.0, 4.5, 9.0] }
        });
        assert_eq!(stream_data(&streams, "time").unwrap(), "[0,1,2]");
        assert!(stream_data(&streams, "watts").is_none());
    }

    #[test]
    fn list_summary_match_skips_unchanged_activity_detail_fetch() {
        let stored = StoredActivitySummary {
            name: "Morning run".to_string(),
            sport_type: Some("Run".to_string()),
            start_date: Some("2026-04-20T12:00:00Z".to_string()),
            elapsed_time_seconds: Some(1900),
            moving_time_seconds: Some(1800),
            distance_meters: Some(8046.72),
            total_elevation_gain: Some(50.0),
            average_heartrate: Some(145.0),
            average_speed: Some(3.8),
            max_speed: Some(5.2),
            average_cadence: Some(82.0),
            average_watts: None,
            kilojoules: None,
            suffer_score: Some(42.0),
            visibility: Some("everyone".to_string()),
            deleted_at: None,
            private_unavailable: 0,
        };
        let activity = json!({
            "id": 100,
            "name": "Morning run",
            "sport_type": "Run",
            "start_date": "2026-04-20T12:00:00Z",
            "elapsed_time": 1900,
            "moving_time": 1800,
            "distance": 8046.72,
            "total_elevation_gain": 50.0,
            "average_heartrate": 145.0,
            "average_speed": 3.8,
            "max_speed": 5.2,
            "average_cadence": 82.0,
            "suffer_score": 42.0,
            "visibility": "everyone"
        });

        assert!(stored_activity_matches_list_summary(&stored, &activity));
    }

    #[test]
    fn list_summary_mismatch_fetches_changed_activity_detail() {
        let stored = StoredActivitySummary {
            name: "Morning run".to_string(),
            sport_type: Some("Run".to_string()),
            start_date: Some("2026-04-20T12:00:00Z".to_string()),
            elapsed_time_seconds: Some(1900),
            moving_time_seconds: Some(1800),
            distance_meters: Some(8046.72),
            total_elevation_gain: Some(50.0),
            average_heartrate: Some(145.0),
            average_speed: Some(3.8),
            max_speed: Some(5.2),
            average_cadence: Some(82.0),
            average_watts: None,
            kilojoules: None,
            suffer_score: Some(42.0),
            visibility: Some("everyone".to_string()),
            deleted_at: None,
            private_unavailable: 0,
        };
        let activity = json!({
            "id": 100,
            "name": "Morning run",
            "sport_type": "Run",
            "start_date": "2026-04-20T12:00:00Z",
            "elapsed_time": 1900,
            "moving_time": 1800,
            "distance": 9000.0
        });

        assert!(!stored_activity_matches_list_summary(&stored, &activity));
    }
}

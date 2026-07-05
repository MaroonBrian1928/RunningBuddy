use crate::{
    auth,
    error::{AppError, Result},
    models::{TrainingAdviceBody, TrainingAdviceResponse, TrainingAdviceRow},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::{Datelike, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;

#[derive(Debug, Deserialize)]
pub struct GenerateAdviceRequest {
    #[serde(default = "default_window")]
    input_window_days: i64,
    activity_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AdviceChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct AdviceChatRequest {
    messages: Vec<AdviceChatMessage>,
}

#[derive(Debug, Serialize)]
pub struct AdviceChatResponse {
    message: String,
}

struct TrainingProfile<'a> {
    plan: Option<&'a str>,
    goals: Option<&'a str>,
    plan_start_date: Option<&'a str>,
}

#[derive(Debug, Clone, FromRow)]
struct AdviceActivityRow {
    id: i64,
    strava_activity_id: i64,
    name: String,
    sport_type: Option<String>,
    start_date: Option<String>,
    start_date_local: Option<String>,
    elapsed_time_seconds: Option<i64>,
    moving_time_seconds: Option<i64>,
    distance_meters: Option<f64>,
    total_elevation_gain: Option<f64>,
    average_heartrate: Option<f64>,
    max_heartrate: Option<f64>,
    average_speed: Option<f64>,
    max_speed: Option<f64>,
    average_cadence: Option<f64>,
    average_watts: Option<f64>,
    kilojoules: Option<f64>,
    suffer_score: Option<f64>,
    raw_activity_json: String,
}

#[derive(Debug, FromRow)]
struct TargetActivityRow {
    id: i64,
    strava_activity_id: i64,
    name: String,
    sport_type: Option<String>,
    start_date: Option<String>,
    start_date_local: Option<String>,
    elapsed_time_seconds: Option<i64>,
    moving_time_seconds: Option<i64>,
    distance_meters: Option<f64>,
    total_elevation_gain: Option<f64>,
    average_heartrate: Option<f64>,
    max_heartrate: Option<f64>,
    average_speed: Option<f64>,
    max_speed: Option<f64>,
    average_cadence: Option<f64>,
    average_watts: Option<f64>,
    kilojoules: Option<f64>,
    suffer_score: Option<f64>,
    raw_activity_json: String,
    time_json: Option<String>,
    distance_json: Option<String>,
    heartrate_json: Option<String>,
    cadence_json: Option<String>,
    velocity_smooth_json: Option<String>,
    watts_json: Option<String>,
    altitude_json: Option<String>,
}

pub async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GenerateAdviceRequest>,
) -> Result<Json<TrainingAdviceResponse>> {
    let user = auth::require_user(&state, &headers).await?;
    let input_window_days = payload.input_window_days.clamp(1, 365);
    let window = recent_activities(&state, input_window_days).await?;
    let target_activity = match payload.activity_id {
        Some(activity_id) => Some(activity_context(&state, activity_id).await?),
        None => None,
    };
    let athlete = athlete_context(&state).await?;
    let profile = TrainingProfile {
        plan: user.training_plan.as_deref(),
        goals: user.training_goals.as_deref(),
        plan_start_date: user.plan_start_date.as_deref(),
    };
    let body = request_advice(
        &state,
        input_window_days,
        &window,
        target_activity.as_ref(),
        athlete.as_ref(),
        &profile,
    )
    .await?;
    let response = persist_advice(&state, input_window_days, payload.activity_id, &body).await?;
    Ok(Json(response))
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TrainingAdviceResponse>>> {
    auth::require_user(&state, &headers).await?;
    let rows = sqlx::query_as::<_, TrainingAdviceRow>(
        "SELECT * FROM training_advice ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(TrainingAdviceRow::into_response)
            .collect(),
    ))
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<TrainingAdviceResponse>> {
    auth::require_user(&state, &headers).await?;
    let row = sqlx::query_as::<_, TrainingAdviceRow>("SELECT * FROM training_advice WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(row.into_response()))
}

pub async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(payload): Json<AdviceChatRequest>,
) -> Result<Json<AdviceChatResponse>> {
    let user = auth::require_user(&state, &headers).await?;
    let row = sqlx::query_as::<_, TrainingAdviceRow>("SELECT * FROM training_advice WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let advice = row.into_response();
    let window = recent_activities(&state, advice.input_window_days.clamp(1, 365)).await?;
    let profile = TrainingProfile {
        plan: user.training_plan.as_deref(),
        goals: user.training_goals.as_deref(),
        plan_start_date: user.plan_start_date.as_deref(),
    };
    let message =
        request_advice_chat(&state, &advice.body, &payload.messages, &window, &profile).await?;
    Ok(Json(AdviceChatResponse { message }))
}

struct ActivityWindow {
    activities: Vec<serde_json::Value>,
    weekly_summary: Vec<serde_json::Value>,
}

async fn recent_activities(state: &AppState, days: i64) -> Result<ActivityWindow> {
    let rows = sqlx::query_as::<_, AdviceActivityRow>(
        r#"
        SELECT id, strava_activity_id, name, sport_type, start_date, start_date_local,
               elapsed_time_seconds, moving_time_seconds, distance_meters,
               total_elevation_gain, average_heartrate, max_heartrate,
               average_speed, max_speed, average_cadence, average_watts,
               kilojoules, suffer_score, raw_activity_json
        FROM activities
        WHERE deleted_at IS NULL
          AND private_unavailable = 0
          AND (start_date IS NULL OR start_date >= datetime('now', ?))
        ORDER BY start_date DESC
        "#,
    )
    .bind(format!("-{days} days"))
    .fetch_all(&state.db)
    .await?;

    Ok(ActivityWindow {
        activities: rows.iter().map(activity_summary).collect(),
        weekly_summary: weekly_training_summary(&rows),
    })
}

fn weekly_training_summary(rows: &[AdviceActivityRow]) -> Vec<serde_json::Value> {
    let mut weeks: std::collections::BTreeMap<NaiveDate, (f64, i64, i64, i64)> =
        std::collections::BTreeMap::new();
    for row in rows {
        let Some(date) = activity_local_date(row.start_date_local.as_deref(), row.start_date.as_deref())
        else {
            continue;
        };
        let week_start = date - Duration::days(date.weekday().num_days_from_monday() as i64);
        let entry = weeks.entry(week_start).or_default();
        if is_run_sport(row.sport_type.as_deref()) {
            entry.0 += row.distance_meters.map(meters_to_miles).unwrap_or(0.0);
            entry.1 += 1;
        } else {
            entry.2 += 1;
        }
        entry.3 += row.moving_time_seconds.unwrap_or(0);
    }
    weeks
        .iter()
        .rev()
        .map(|(week_start, (run_miles, run_count, other_count, moving_seconds))| {
            json!({
                "week_starting_monday": week_start.to_string(),
                "run_miles": (run_miles * 10.0).round() / 10.0,
                "run_count": run_count,
                "other_activity_count": other_count,
                "total_moving_time_seconds": moving_seconds,
            })
        })
        .collect()
}

fn activity_local_date(
    start_date_local: Option<&str>,
    start_date_utc: Option<&str>,
) -> Option<NaiveDate> {
    let value = start_date_local.or(start_date_utc)?;
    NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok()
}

async fn activity_context(state: &AppState, activity_id: i64) -> Result<serde_json::Value> {
    let row = sqlx::query_as::<_, TargetActivityRow>(
        r#"
        SELECT a.id, a.strava_activity_id, a.name, a.sport_type, a.start_date, a.start_date_local,
               a.elapsed_time_seconds, a.moving_time_seconds, a.distance_meters,
               a.total_elevation_gain, a.average_heartrate, a.max_heartrate,
               a.average_speed, a.max_speed, a.average_cadence, a.average_watts,
               a.kilojoules, a.suffer_score, a.raw_activity_json,
               s.time_json, s.distance_json, s.heartrate_json, s.cadence_json,
               s.velocity_smooth_json, s.watts_json, s.altitude_json
        FROM activities a
        LEFT JOIN activity_streams s ON s.activity_id = a.id
        WHERE a.id = ?
          AND a.deleted_at IS NULL
          AND a.private_unavailable = 0
        "#,
    )
    .bind(activity_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(json!({
        "summary": target_activity_summary(&row),
        "stream_summary": stream_summary(&row),
    }))
}

async fn athlete_context(state: &AppState) -> Result<Option<serde_json::Value>> {
    let row = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT a.raw_profile_json
        FROM athletes a
        JOIN strava_tokens t ON t.athlete_id = a.id
        ORDER BY t.updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    Ok(row
        .and_then(|row| serde_json::from_str::<serde_json::Value>(&row.0).ok())
        .map(compact_athlete_profile))
}

fn compact_athlete_profile(profile: serde_json::Value) -> serde_json::Value {
    json!({
        "id": profile.get("id"),
        "username": profile.get("username"),
        "firstname": profile.get("firstname"),
        "lastname": profile.get("lastname"),
        "city": profile.get("city"),
        "state": profile.get("state"),
        "country": profile.get("country"),
        "sex": profile.get("sex"),
        "weight_kg": profile.get("weight"),
        "ftp": profile.get("ftp"),
        "measurement_preference": profile.get("measurement_preference"),
        "shoes": compact_gear(profile.get("shoes")),
        "bikes": compact_gear(profile.get("bikes")),
    })
}

fn compact_gear(gear: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(items) = gear.and_then(|gear| gear.as_array()) else {
        return json!([]);
    };

    json!(items
        .iter()
        .map(|item| {
            let distance_meters = item.get("distance").and_then(|value| value.as_f64());
            json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "primary": item.get("primary"),
                "resource_state": item.get("resource_state"),
                "distance_meters": distance_meters,
                "distance_miles": distance_meters.map(meters_to_miles),
            })
        })
        .collect::<Vec<_>>())
}

fn compact_activity_gear(activity: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let activity = activity?;
    let gear = activity.get("gear")?;
    Some(json!({
        "gear_id": activity.get("gear_id"),
        "id": gear.get("id"),
        "name": gear.get("name"),
        "primary": gear.get("primary"),
        "resource_state": gear.get("resource_state"),
        "distance_meters": gear.get("distance").and_then(|value| value.as_f64()),
        "distance_miles": gear.get("distance").and_then(|value| value.as_f64()).map(meters_to_miles),
    }))
}

fn activity_summary(row: &AdviceActivityRow) -> serde_json::Value {
    let raw = serde_json::from_str::<serde_json::Value>(&row.raw_activity_json).ok();
    let local_date = activity_local_date(row.start_date_local.as_deref(), row.start_date.as_deref());
    json!({
        "id": row.id,
        "strava_activity_id": row.strava_activity_id,
        "name": row.name,
        "sport_type": row.sport_type,
        "start_date_utc": row.start_date,
        "start_date_local": row.start_date_local,
        "local_date": local_date.map(|date| date.to_string()),
        "local_day_of_week": local_date.map(|date| date.format("%A").to_string()),
        "elapsed_time_seconds": row.elapsed_time_seconds,
        "moving_time_seconds": row.moving_time_seconds,
        "distance_meters": row.distance_meters,
        "distance_miles": row.distance_meters.map(meters_to_miles),
        "pace_seconds_per_mile": pace_seconds_per_mile(row.distance_meters, row.moving_time_seconds),
        "total_elevation_gain_meters": row.total_elevation_gain,
        "average_heartrate": row.average_heartrate,
        "max_heartrate": row.max_heartrate,
        "average_speed_mps": row.average_speed,
        "max_speed_mps": row.max_speed,
        "average_cadence": row.average_cadence,
        "average_run_cadence_spm": run_cadence(row.sport_type.as_deref(), row.average_cadence),
        "average_watts": row.average_watts,
        "kilojoules": row.kilojoules,
        "relative_effort": row.suffer_score,
        "perceived_exertion": raw.as_ref().and_then(|activity| activity.get("perceived_exertion")),
        "description": raw.as_ref().and_then(|activity| activity.get("description")),
        "workout_type": raw.as_ref().and_then(|activity| activity.get("workout_type")),
        "gear_id": raw.as_ref().and_then(|activity| activity.get("gear_id")),
        "gear": compact_activity_gear(raw.as_ref()),
    })
}

fn target_activity_summary(row: &TargetActivityRow) -> serde_json::Value {
    let raw = serde_json::from_str::<serde_json::Value>(&row.raw_activity_json).ok();
    let local_date = activity_local_date(row.start_date_local.as_deref(), row.start_date.as_deref());
    json!({
        "id": row.id,
        "strava_activity_id": row.strava_activity_id,
        "name": row.name,
        "sport_type": row.sport_type,
        "start_date_utc": row.start_date,
        "start_date_local": row.start_date_local,
        "local_date": local_date.map(|date| date.to_string()),
        "local_day_of_week": local_date.map(|date| date.format("%A").to_string()),
        "elapsed_time_seconds": row.elapsed_time_seconds,
        "moving_time_seconds": row.moving_time_seconds,
        "distance_meters": row.distance_meters,
        "distance_miles": row.distance_meters.map(meters_to_miles),
        "pace_seconds_per_mile": pace_seconds_per_mile(row.distance_meters, row.moving_time_seconds),
        "total_elevation_gain_meters": row.total_elevation_gain,
        "average_heartrate": row.average_heartrate,
        "max_heartrate": row.max_heartrate,
        "average_speed_mps": row.average_speed,
        "max_speed_mps": row.max_speed,
        "average_cadence": row.average_cadence,
        "average_run_cadence_spm": run_cadence(row.sport_type.as_deref(), row.average_cadence),
        "average_watts": row.average_watts,
        "kilojoules": row.kilojoules,
        "relative_effort": row.suffer_score,
        "perceived_exertion": raw.as_ref().and_then(|activity| activity.get("perceived_exertion")),
        "description": raw.as_ref().and_then(|activity| activity.get("description")),
        "workout_type": raw.as_ref().and_then(|activity| activity.get("workout_type")),
        "gear_id": raw.as_ref().and_then(|activity| activity.get("gear_id")),
        "gear": compact_activity_gear(raw.as_ref()),
        "splits_metric": raw.as_ref().and_then(|activity| activity.get("splits_metric")),
        "splits_standard": raw.as_ref().and_then(|activity| activity.get("splits_standard")),
        "best_efforts": raw.as_ref().and_then(|activity| activity.get("best_efforts")),
        "laps": raw.as_ref().and_then(|activity| activity.get("laps")),
    })
}

fn stream_summary(row: &TargetActivityRow) -> serde_json::Value {
    let time = parse_number_stream(row.time_json.as_deref());
    let distance = parse_number_stream(row.distance_json.as_deref());
    let heartrate = parse_number_stream(row.heartrate_json.as_deref());
    let cadence = parse_number_stream(row.cadence_json.as_deref());
    let velocity = parse_number_stream(row.velocity_smooth_json.as_deref());
    let watts = parse_number_stream(row.watts_json.as_deref());
    let altitude = parse_number_stream(row.altitude_json.as_deref());

    json!({
        "sample_count": time.len().max(distance.len()).max(heartrate.len()).max(cadence.len()).max(velocity.len()).max(watts.len()).max(altitude.len()),
        "time_seconds": number_stats(&time),
        "distance_meters": number_stats(&distance),
        "heartrate_bpm": number_stats(&heartrate),
        "cadence": number_stats(&cadence),
        "run_cadence_spm": if is_run_sport(row.sport_type.as_deref()) {
            number_stats(&cadence.iter().map(|value| value * 2.0).collect::<Vec<_>>())
        } else {
            serde_json::Value::Null
        },
        "velocity_smooth_mps": number_stats(&velocity),
        "pace_seconds_per_mile": pace_stats_from_velocity(&velocity),
        "watts": number_stats(&watts),
        "altitude_meters": number_stats(&altitude),
        "duration_seconds": time.last().copied(),
        "distance_total_meters": distance.last().copied(),
    })
}

fn parse_number_stream(value: Option<&str>) -> Vec<f64> {
    value
        .and_then(|value| serde_json::from_str::<Vec<f64>>(value).ok())
        .unwrap_or_default()
}

fn number_stats(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }

    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for value in values {
        min = min.min(*value);
        max = max.max(*value);
        sum += value;
    }

    json!({
        "count": values.len(),
        "first": values.first().copied(),
        "last": values.last().copied(),
        "min": min,
        "max": max,
        "avg": sum / values.len() as f64,
    })
}

fn pace_stats_from_velocity(values: &[f64]) -> serde_json::Value {
    let paces = values
        .iter()
        .filter(|value| **value > 0.0)
        .map(|meters_per_second| 1609.344 / meters_per_second)
        .collect::<Vec<_>>();
    number_stats(&paces)
}

fn meters_to_miles(value: f64) -> f64 {
    value / 1609.344
}

fn pace_seconds_per_mile(
    distance_meters: Option<f64>,
    moving_time_seconds: Option<i64>,
) -> Option<f64> {
    let distance_meters = distance_meters?;
    let moving_time_seconds = moving_time_seconds?;
    if distance_meters <= 0.0 || moving_time_seconds <= 0 {
        return None;
    }
    Some(moving_time_seconds as f64 / meters_to_miles(distance_meters))
}

fn is_run_sport(sport_type: Option<&str>) -> bool {
    // Matches Run, TrailRun, VirtualRun, etc.
    sport_type
        .map(|sport_type| sport_type.to_ascii_lowercase().contains("run"))
        .unwrap_or(false)
}

fn run_cadence(sport_type: Option<&str>, cadence: Option<f64>) -> Option<f64> {
    cadence.map(|cadence| {
        if is_run_sport(sport_type) {
            cadence * 2.0
        } else {
            cadence
        }
    })
}

async fn request_advice(
    state: &AppState,
    input_window_days: i64,
    window: &ActivityWindow,
    target_activity: Option<&serde_json::Value>,
    athlete: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    if window.activities.is_empty() && target_activity.is_none() {
        tracing::info!("no activities available, using local fallback advice");
        return Ok(local_fallback_advice(
            input_window_days,
            target_activity.is_some(),
        ));
    }

    tracing::info!(
        provider = state.config.llm_provider,
        model = state.config.llm_model,
        activities_count = window.activities.len(),
        has_target_activity = target_activity.is_some(),
        has_athlete_profile = athlete.is_some(),
        has_training_plan = profile.plan.is_some(),
        has_training_goals = profile.goals.is_some(),
        "requesting training advice"
    );

    match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_advice(
                state,
                input_window_days,
                window,
                target_activity,
                athlete,
                profile,
            )
            .await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_advice(
                state,
                input_window_days,
                window,
                target_activity,
                athlete,
                profile,
            )
            .await
        }
        provider => {
            tracing::warn!(
                provider = provider,
                "unknown or unconfigured LLM provider, using local fallback advice"
            );
            Ok(local_fallback_advice(
                input_window_days,
                target_activity.is_some(),
            ))
        }
    }
}

async fn openai_advice(
    state: &AppState,
    input_window_days: i64,
    window: &ActivityWindow,
    target_activity: Option<&serde_json::Value>,
    athlete: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let scope = advice_request_scope(target_activity);
    let user_content = build_user_content(
        scope,
        input_window_days,
        window,
        target_activity,
        athlete,
        profile,
    );

    let payload = json!({
        "model": state.config.llm_model,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "training_advice",
                "strict": true,
                "schema": advice_response_schema(),
            }
        },
        "messages": [
            { "role": "system", "content": advice_system_prompt(scope) },
            { "role": "user", "content": user_content.to_string() }
        ]
    });
    let response_result = state
        .http
        .post(format!(
            "{}/chat/completions",
            state.config.openai_base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await;

    let response = match response_result {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = %err, "failed to send request to OpenAI API");
            return Err(err.into());
        }
    };

    let response = match response.error_for_status() {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = %err, "OpenAI API returned an error status");
            return Err(err.into());
        }
    };

    let response_json: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(err) => {
            tracing::error!(error = %err, "failed to parse OpenAI response as JSON");
            return Err(err.into());
        }
    };

    let content = response_json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            tracing::error!("OpenAI response did not include JSON content");
            AppError::BadRequest("OpenAI response did not include JSON content".into())
        })?;
    parse_advice_body(content)
}

async fn gemini_advice(
    state: &AppState,
    input_window_days: i64,
    window: &ActivityWindow,
    target_activity: Option<&serde_json::Value>,
    athlete: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let scope = advice_request_scope(target_activity);
    let user_content = build_user_content(
        scope,
        input_window_days,
        window,
        target_activity,
        athlete,
        profile,
    );

    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!("{}\n\n{}", advice_system_prompt(scope), user_content) }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": advice_response_schema_gemini(),
        }
    });
    let response_result = state
        .http
        .post(format!(
            "{}/models/{}:generateContent?key={}",
            state.config.gemini_base_url.trim_end_matches('/'),
            state.config.llm_model,
            api_key
        ))
        .json(&payload)
        .send()
        .await;

    let response = match response_result {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = %err, "failed to send request to Gemini API");
            return Err(err.into());
        }
    };

    let response = match response.error_for_status() {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = %err, "Gemini API returned an error status");
            return Err(err.into());
        }
    };

    let response_json: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(err) => {
            tracing::error!(error = %err, "failed to parse Gemini response as JSON");
            return Err(err.into());
        }
    };

    let content = response_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            tracing::error!("Gemini response did not include JSON content");
            AppError::BadRequest("Gemini response did not include JSON content".into())
        })?;
    parse_advice_body(content)
}

fn parse_advice_body(content: &str) -> Result<TrainingAdviceBody> {
    let clean_content = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(clean_content).map_err(|err| AppError::BadRequest(err.to_string()))
}

async fn request_advice_chat(
    state: &AppState,
    advice: &TrainingAdviceBody,
    messages: &[AdviceChatMessage],
    window: &ActivityWindow,
    profile: &TrainingProfile<'_>,
) -> Result<String> {
    match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_advice_chat(state, advice, messages, window, profile).await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_advice_chat(state, advice, messages, window, profile).await
        }
        _ => Ok(local_chat_fallback(advice, messages)),
    }
}

fn advice_chat_payload(
    advice: &TrainingAdviceBody,
    messages: &[AdviceChatMessage],
    window: &ActivityWindow,
    profile: &TrainingProfile<'_>,
) -> serde_json::Value {
    let today = Local::now().date_naive();
    json!({
        "current_date": today.to_string(),
        "current_day_of_week": today.format("%A").to_string(),
        "data_notes": DATA_NOTES,
        "training_profile": {
            "plan": profile.plan,
            "goals": profile.goals,
            "plan_start_date": profile.plan_start_date,
            "progress": plan_progress(profile, today),
        },
        "activities": window.activities,
        "weekly_training_summary": window.weekly_summary,
        "saved_advice": advice_chat_context(advice),
        "conversation": messages,
    })
}

async fn openai_advice_chat(
    state: &AppState,
    advice: &TrainingAdviceBody,
    messages: &[AdviceChatMessage],
    window: &ActivityWindow,
    profile: &TrainingProfile<'_>,
) -> Result<String> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let chat_messages = json!([
        { "role": "system", "content": advice_chat_system_prompt() },
        { "role": "user", "content": advice_chat_payload(advice, messages, window, profile).to_string() }
    ]);
    let payload = json!({
        "model": state.config.llm_model,
        "messages": chat_messages
    });
    let response_json: serde_json::Value = state
        .http
        .post(format!(
            "{}/chat/completions",
            state.config.openai_base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    response_json["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("OpenAI response did not include chat content".into()))
}

async fn gemini_advice_chat(
    state: &AppState,
    advice: &TrainingAdviceBody,
    messages: &[AdviceChatMessage],
    window: &ActivityWindow,
    profile: &TrainingProfile<'_>,
) -> Result<String> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!(
                "{}\n\n{}",
                advice_chat_system_prompt(),
                advice_chat_payload(advice, messages, window, profile)
            ) }]
        }]
    });
    let response_json: serde_json::Value = state
        .http
        .post(format!(
            "{}/models/{}:generateContent?key={}",
            state.config.gemini_base_url.trim_end_matches('/'),
            state.config.llm_model,
            api_key
        ))
        .json(&payload)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    response_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("Gemini response did not include chat content".into()))
}

fn advice_chat_context(advice: &TrainingAdviceBody) -> serde_json::Value {
    json!({
        "summary": advice.summary,
        "load_observations": advice.load_observations,
        "risks": advice.risks,
        "next_7_days": advice.next_7_days,
        "recovery_notes": advice.recovery_notes,
        "confidence": advice.confidence,
    })
}

fn local_chat_fallback(advice: &TrainingAdviceBody, messages: &[AdviceChatMessage]) -> String {
    let question = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("that");
    format!(
        "I cannot reach a configured coach model right now. Based on the saved advice, keep this anchored on: {} For your follow-up about \"{}\", stay conservative and use the next planned easy/recovery guidance until the model is configured.",
        advice.summary, question
    )
}

async fn persist_advice(
    state: &AppState,
    input_window_days: i64,
    activity_id: Option<i64>,
    body: &TrainingAdviceBody,
) -> Result<TrainingAdviceResponse> {
    let row = sqlx::query_as::<_, TrainingAdviceRow>(
        r#"
        INSERT INTO training_advice
            (activity_id, provider, model, input_window_days, summary, load_observations_json,
             risks_json, next_7_days_json, recovery_notes, confidence,
             raw_response_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(activity_id)
    .bind(&state.config.llm_provider)
    .bind(&state.config.llm_model)
    .bind(input_window_days)
    .bind(&body.summary)
    .bind(serde_json::to_string(&body.load_observations).unwrap())
    .bind(serde_json::to_string(&body.risks).unwrap())
    .bind(serde_json::to_string(&body.next_7_days).unwrap())
    .bind(&body.recovery_notes)
    .bind(body.confidence)
    .bind(serde_json::to_string(body).unwrap())
    .fetch_one(&state.db)
    .await?;
    Ok(row.into_response())
}

fn local_fallback_advice(input_window_days: i64, has_target_activity: bool) -> TrainingAdviceBody {
    let (summary, load_observations, next_7_days) = if has_target_activity {
        (
            format!(
                "No configured LLM response is available to review the selected activity against your plan and the last {input_window_days} days of training yet."
            ),
            vec![
                "Once a coach model is configured, this view should focus on what this specific activity says about execution, fatigue, pacing, and plan fit.".into(),
            ],
            vec![
                "Treat the next planned workout conservatively until the app can compare this activity with your plan and recent load.".into(),
                "If the selected effort felt unusually hard, bias the next run easier or shorter.".into(),
            ],
        )
    } else {
        (
            format!(
                "No configured LLM response is available for recent training across the last {input_window_days} days yet."
            ),
            vec![
                "Connect Strava and sync activities to build a useful training load picture.".into(),
            ],
            vec![
                "Keep most running easy until recent activity history is synced.".into(),
                "Add one rest or mobility-focused day if soreness is elevated.".into(),
            ],
        )
    };

    TrainingAdviceBody {
        summary,
        load_observations,
        risks: vec![
            "Avoid ramping volume or intensity sharply while the app has limited history.".into(),
        ],
        next_7_days,
        recovery_notes: "Prioritize sleep, hydration, and easy aerobic consistency.".into(),
        confidence: 0.2,
    }
}

fn advice_request_scope(target_activity: Option<&serde_json::Value>) -> &'static str {
    if target_activity.is_some() {
        "activity_review"
    } else {
        "training_overview"
    }
}

const COACH_PERSONA: &str = r#"You are acting as an experienced running coach focused on practical, evidence-based training guidance.

Your coaching style should be:

* Direct, precise, and honest
* Focused on training outcomes, recovery, injury prevention, and long-term consistency
* Skeptical of vague assumptions and quick fixes
* Willing to challenge poor decisions (skipping recovery, running too hard too often, unrealistic pacing, etc.)
* Structured and specific rather than motivational fluff

Do not:

* Give generic "listen to your body" advice without specifics
* Default to encouragement over accuracy
* Recommend increasing intensity without strong justification"#;

const JSON_CONTRACT: &str = r#"Return strict JSON matching this structure exactly: { "summary": string, "load_observations": string[], "risks": string[], "next_7_days": string[], "recovery_notes": string, "confidence": number between 0 and 1 }.
Write the JSON values in a conversational coaching voice, as if speaking directly to the runner."#;

fn advice_system_prompt(scope: &str) -> &'static str {
    match scope {
        "activity_review" => activity_review_system_prompt(),
        _ => training_overview_system_prompt(),
    }
}

fn activity_review_system_prompt() -> &'static str {
    static PROMPT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PROMPT.get_or_init(|| {
        format!(
            r#"{persona}

Your job is to review target_activity. The plan and recent activities are context only; do not review the plan by itself.

When responding, apply these rules only when they help explain target_activity:

* Translate what happened in target_activity into actionable pacing, effort, heart rate, or RPE guidance
* Explain what role this workout most likely played (easy run, tempo, intervals, long run, recovery, etc.) and whether the execution matched that purpose
* Suggest modifications to upcoming workouts only when this activity justifies them, and explain why
* Flag overtraining risk, poor progression, or likely injury traps that this activity reveals
* If information is missing, ask targeted follow-up questions instead of assuming

Dates: read data_notes and use each activity's local_date and local_day_of_week (the athlete's wall clock) to decide which calendar day it happened; start_date_utc can fall on a different day. current_date and current_day_of_week are the athlete's local today.

Use athlete_profile, training_profile, activities, weekly_training_summary, and target_activity when provided. The athlete_profile includes compact Strava athlete details and gear mileage when available. The activities list contains compact summaries for all synced, available activities in the requested window, including cross-training when present. Use weekly_training_summary for weekly mileage totals rather than recomputing them yourself. The target_activity contains the selected activity summary (including splits, laps, and best efforts when available) and compact stream summaries.

* Make target_activity the center of the answer. The advice should read like a review of that exact run, not a general training-plan check-in.
* Every field must be either about target_activity itself or about target_activity in the context of the plan.
* Use the training plan, goals, plan start date, and recent activities only to judge this activity's intended purpose, execution, load progression, recovery impact, and next-workout implications.
* Tie every summary, observation, risk, recovery note, and next-7-day suggestion back to evidence from target_activity when possible: distance, duration, pacing/speed, heart rate, elevation, cadence, sport type, start date, relative effort, and how it compares with recent activities.
* Do not include standalone feedback on whether the plan is good, bad, aggressive, conservative, incomplete, or internally inconsistent.
* Do not discuss general training strategy unless it directly explains this activity or the adjustment needed because of this activity.
* Do not fill missing activity details with broader plan commentary.
* If target_activity lacks key data, say what is missing and give the narrowest useful activity-specific guidance rather than expanding into generic plan advice.

{contract}"#,
            persona = COACH_PERSONA,
            contract = JSON_CONTRACT,
        )
    }).as_str()
}

fn training_overview_system_prompt() -> &'static str {
    static PROMPT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PROMPT.get_or_init(|| {
        format!(
            r#"{persona}

Your job is to help the runner successfully complete the training plan they provide. Your role is not to blindly repeat the plan, but to interpret it, explain it, adapt it when needed, and help the runner execute it consistently and safely.

When responding:

* Treat current_date and current_day_of_week as the runner's local today and use them, together with training_profile.progress (days_since_plan_start, plan_week_number), to anchor where the runner is in the plan, the recent activity window, and the next 7 days.
* Read data_notes. Each activity's local_date and local_day_of_week give the runner's calendar day; start_date_utc is UTC and can fall on a different day. Always match plan days against local_date, never against the UTC timestamp.
* Only describe a planned workout as skipped or missed when no activity exists on that local calendar date AND the date is fully in the past. Never call today's planned workout skipped: the day is not over and activities can sync late. If an activity on the right day roughly matches the planned distance or effort, treat the workout as completed, not missed.
* Use weekly_training_summary for weekly mileage totals and activity counts rather than recomputing them from raw activities.
* First understand the full training plan, goal race/event, timeline, current fitness level, injury history, available training days, and constraints (work, family, travel, equipment, terrain, weather)
* Help translate workouts into actionable pacing, effort, heart rate, or RPE guidance
* Explain the purpose of each workout (easy run, tempo, intervals, long run, recovery, deload, etc.)
* Identify if the plan is too aggressive, too conservative, or internally inconsistent
* Suggest modifications only when justified, and explain why
* Prioritize consistency over hero workouts
* Consider sleep, nutrition, hydration, fueling, strength work, and recovery as part of the plan
* Flag overtraining risk, poor progression, or likely injury traps early
* If information is missing, ask targeted follow-up questions instead of assuming
* Review the recent activity set and plan as a whole.
* Keep the guidance focused on load, progression, risk, and the next week.
* Point out specific patterns in the data that inform the advice.
* Use the data to support your recommendations and avoid making assumptions.

Also do not assume every plan is well-designed.

Use current_date, athlete_profile, training_profile, activities, and weekly_training_summary when provided. The athlete_profile includes compact Strava athlete details and gear mileage when available. The activities list contains compact summaries for all synced, available activities in the requested window, including cross-training when present.

If the training plan, goal race, timeline, current fitness, injury history, available days, or constraints are missing, include concise targeted questions inside the relevant JSON fields instead of inventing details.

{contract}"#,
            persona = COACH_PERSONA,
            contract = JSON_CONTRACT,
        )
    }).as_str()
}

fn advice_chat_system_prompt() -> &'static str {
    "You are a running coach continuing a conversation about previously generated training advice. Answer conversationally in plain text, not JSON. Be specific, concise, and practical. Keep replies under 150 words unless the user explicitly asks for a plan, table, or longer breakdown. Use saved_advice as the starting point, but ground answers in the provided training_profile, activities, and weekly_training_summary — they are the current data and win over saved_advice if the two disagree. Read data_notes: use each activity's local_date to decide which calendar day it happened, and treat current_date as the runner's local today. Answer the latest user follow-up, and ask one targeted question when key information is missing. Do not add repeated safety disclaimers; the app displays one shared footer disclaimer."
}

const DATA_NOTES: &str = "All start_date_local, local_date, and local_day_of_week values are the athlete's wall-clock time; start_date_utc is UTC and can fall on a different calendar day. Use local_date to decide which day an activity happened. current_date/current_day_of_week are the athlete's local today. Activities can sync with a delay, so today may look incomplete.";

fn build_user_content(
    scope: &str,
    input_window_days: i64,
    window: &ActivityWindow,
    target_activity: Option<&serde_json::Value>,
    athlete: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> serde_json::Value {
    let now = Local::now();
    let today = now.date_naive();
    let mut content = json!({
        "advice_request_scope": scope,
        "current_date": today.to_string(),
        "current_day_of_week": today.format("%A").to_string(),
        "utc_offset": now.format("%:z").to_string(),
        "data_notes": DATA_NOTES,
        "input_window_days": input_window_days,
        "activities": window.activities,
        "weekly_training_summary": window.weekly_summary,
        "athlete_profile": athlete,
        "training_profile": {
            "plan": profile.plan,
            "goals": profile.goals,
            "plan_start_date": profile.plan_start_date,
            "progress": plan_progress(profile, today),
        },
    });
    if let Some(activity) = target_activity {
        content["target_activity"] = activity.clone();
    }
    content
}

fn plan_progress(profile: &TrainingProfile<'_>, today: NaiveDate) -> serde_json::Value {
    let Some(start) = profile
        .plan_start_date
        .and_then(|date| NaiveDate::parse_from_str(date.get(..10).unwrap_or(date), "%Y-%m-%d").ok())
    else {
        return serde_json::Value::Null;
    };
    let days = (today - start).num_days();
    json!({
        "days_since_plan_start": days,
        "plan_week_number": if days >= 0 { Some(days / 7 + 1) } else { None },
    })
}

fn advice_response_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "load_observations", "risks", "next_7_days", "recovery_notes", "confidence"],
        "properties": {
            "summary": { "type": "string" },
            "load_observations": { "type": "array", "items": { "type": "string" } },
            "risks": { "type": "array", "items": { "type": "string" } },
            "next_7_days": { "type": "array", "items": { "type": "string" } },
            "recovery_notes": { "type": "string" },
            "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
        }
    })
}

fn advice_response_schema_gemini() -> serde_json::Value {
    json!({
        "type": "OBJECT",
        "required": ["summary", "load_observations", "risks", "next_7_days", "recovery_notes", "confidence"],
        "properties": {
            "summary": { "type": "STRING" },
            "load_observations": { "type": "ARRAY", "items": { "type": "STRING" } },
            "risks": { "type": "ARRAY", "items": { "type": "STRING" } },
            "next_7_days": { "type": "ARRAY", "items": { "type": "STRING" } },
            "recovery_notes": { "type": "STRING" },
            "confidence": { "type": "NUMBER" }
        }
    })
}

fn default_window() -> i64 {
    28
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_training_advice_json() {
        let body = parse_advice_body(
            r#"{
                "summary":"steady",
                "load_observations":["easy volume"],
                "risks":["none obvious"],
                "next_7_days":["easy run"],
                "recovery_notes":"sleep",
                "confidence":0.7
            }"#,
        )
        .unwrap();

        assert_eq!(body.summary, "steady");
        assert_eq!(body.next_7_days.len(), 1);
    }

    #[test]
    fn scoped_advice_prompts_require_json_response() {
        assert!(activity_review_system_prompt().contains("Return strict JSON"));
        assert!(training_overview_system_prompt().contains("Return strict JSON"));
    }

    #[test]
    fn run_cadence_doubles_for_all_run_variants() {
        assert_eq!(run_cadence(Some("Run"), Some(85.0)), Some(170.0));
        assert_eq!(run_cadence(Some("TrailRun"), Some(85.0)), Some(170.0));
        assert_eq!(run_cadence(Some("VirtualRun"), Some(85.0)), Some(170.0));
        assert_eq!(run_cadence(Some("Ride"), Some(85.0)), Some(85.0));
        assert_eq!(run_cadence(None, Some(85.0)), Some(85.0));
    }

    #[test]
    fn activity_local_date_prefers_local_over_utc() {
        let date = activity_local_date(Some("2026-07-04T20:15:00Z"), Some("2026-07-05T01:15:00Z"));
        assert_eq!(date.unwrap().to_string(), "2026-07-04");

        let fallback = activity_local_date(None, Some("2026-07-05T01:15:00Z"));
        assert_eq!(fallback.unwrap().to_string(), "2026-07-05");

        assert!(activity_local_date(None, None).is_none());
    }

    #[test]
    fn weekly_training_summary_groups_runs_by_local_week() {
        let base = AdviceActivityRow {
            id: 1,
            strava_activity_id: 1,
            name: "Run".into(),
            sport_type: Some("Run".into()),
            start_date: None,
            start_date_local: Some("2026-06-29T07:00:00Z".into()), // Monday
            elapsed_time_seconds: None,
            moving_time_seconds: Some(1800),
            distance_meters: Some(1609.344 * 4.0),
            total_elevation_gain: None,
            average_heartrate: None,
            max_heartrate: None,
            average_speed: None,
            max_speed: None,
            average_cadence: None,
            average_watts: None,
            kilojoules: None,
            suffer_score: None,
            raw_activity_json: "{}".into(),
        };
        let sunday_run = AdviceActivityRow {
            id: 2,
            strava_activity_id: 2,
            start_date_local: Some("2026-07-05T07:00:00Z".into()), // Sunday, same week
            distance_meters: Some(1609.344 * 6.0),
            ..base.clone()
        };
        let ride = AdviceActivityRow {
            id: 3,
            strava_activity_id: 3,
            sport_type: Some("Ride".into()),
            start_date_local: Some("2026-07-01T07:00:00Z".into()),
            ..base.clone()
        };

        let summary = weekly_training_summary(&[base, sunday_run, ride]);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0]["week_starting_monday"], "2026-06-29");
        assert_eq!(summary[0]["run_miles"], 10.0);
        assert_eq!(summary[0]["run_count"], 2);
        assert_eq!(summary[0]["other_activity_count"], 1);
    }

    #[test]
    fn plan_progress_computes_week_number() {
        let profile = TrainingProfile {
            plan: None,
            goals: None,
            plan_start_date: Some("2026-06-15"),
        };
        let today = NaiveDate::from_ymd_opt(2026, 7, 4).unwrap();
        let progress = plan_progress(&profile, today);
        assert_eq!(progress["days_since_plan_start"], 19);
        assert_eq!(progress["plan_week_number"], 3);

        let none = TrainingProfile { plan: None, goals: None, plan_start_date: None };
        assert!(plan_progress(&none, today).is_null());
    }

    #[test]
    fn compact_athlete_profile_includes_gear_mileage() {
        let profile = json!({
            "id": 123,
            "firstname": "Marianne",
            "shoes": [{
                "id": "g123",
                "primary": true,
                "name": "adidas",
                "distance": 4904.0
            }],
            "bikes": [{
                "id": "b123",
                "primary": true,
                "name": "EMC",
                "distance": 1609.344
            }]
        });

        let compact = compact_athlete_profile(profile);

        assert_eq!(compact["shoes"][0]["name"], "adidas");
        assert_eq!(compact["shoes"][0]["distance_meters"], 4904.0);
        assert!((compact["shoes"][0]["distance_miles"].as_f64().unwrap() - 3.047).abs() < 0.001);
        assert_eq!(compact["bikes"][0]["name"], "EMC");
        assert_eq!(compact["bikes"][0]["distance_miles"], 1.0);
    }
}

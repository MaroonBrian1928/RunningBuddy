use crate::{
    auth,
    error::{AppError, Result},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

const STRAVA_API_BASE: &str = "https://www.strava.com/api/v3";

#[derive(Debug, Deserialize)]
pub struct WebhookChallenge {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub object_type: String,
    pub object_id: i64,
    pub aspect_type: String,
    pub owner_id: i64,
    pub subscription_id: Option<i64>,
    #[serde(default)]
    pub updates: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    authorization_url: String,
}

#[derive(Debug, Serialize)]
pub struct StravaStatusResponse {
    configured: bool,
    connected: bool,
    athlete: Option<ConnectedAthlete>,
    scopes: Vec<String>,
    token_expires_at: Option<i64>,
    queued_jobs: i64,
    running_jobs: i64,
    failed_jobs: i64,
    last_completed_sync_at: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ConnectedAthlete {
    strava_athlete_id: i64,
    username: Option<String>,
    firstname: Option<String>,
    lastname: Option<String>,
    profile_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    scope: Option<String>,
    error: Option<String>,
}

pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ConnectResponse>> {
    auth::require_user(&state, &headers).await?;
    let client_id = state
        .config
        .strava_client_id
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("STRAVA_CLIENT_ID is not configured".into()))?;
    let mut url = Url::parse("https://www.strava.com/oauth/authorize")
        .map_err(|err| AppError::BadRequest(err.to_string()))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair(
            "redirect_uri",
            &format!("{}/api/strava/callback", state.config.public_url),
        )
        .append_pair("approval_prompt", "auto")
        .append_pair("scope", "read,activity:read,activity:read_all");
    Ok(Json(ConnectResponse {
        authorization_url: url.to_string(),
    }))
}

pub async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StravaStatusResponse>> {
    auth::require_user(&state, &headers).await?;
    let token_row = sqlx::query_as::<_, (i64, String, i64)>(
        r#"
        SELECT athlete_id, scopes, expires_at
        FROM strava_tokens
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    let athlete = if let Some((athlete_id, _, _)) = token_row.as_ref() {
        sqlx::query_as::<_, ConnectedAthlete>(
            r#"
            SELECT strava_athlete_id, username, firstname, lastname, profile_url
            FROM athletes
            WHERE id = ?
            "#,
        )
        .bind(athlete_id)
        .fetch_optional(&state.db)
        .await?
    } else {
        None
    };

    let counts = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN status = 'queued' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0)
        FROM sync_jobs
        "#,
    )
    .fetch_one(&state.db)
    .await?;
    let last_completed_sync_at = sqlx::query_as::<_, (String,)>(
        "SELECT updated_at FROM sync_jobs WHERE status = 'completed' ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?
    .map(|row| row.0);

    let scopes = token_row
        .as_ref()
        .map(|(_, scopes, _)| parse_scopes(scopes))
        .unwrap_or_default();

    Ok(Json(StravaStatusResponse {
        configured: (state.config.strava_client_id.is_some()
            && state.config.strava_client_secret.is_some())
            || (state.config.strava_access_token.is_some()
                && state.config.strava_refresh_token.is_some()),
        connected: token_row.is_some(),
        athlete,
        scopes,
        token_expires_at: token_row.map(|(_, _, expires_at)| expires_at),
        queued_jobs: counts.0,
        running_jobs: counts.1,
        failed_jobs: counts.2,
        last_completed_sync_at,
    }))
}

pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Result<impl IntoResponse> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!(
            "Strava rejected authorization: {error}"
        )));
    }
    let code = query
        .code
        .ok_or_else(|| AppError::BadRequest("missing Strava code".into()))?;
    exchange_code(&state, &code, query.scope.unwrap_or_default()).await?;
    Ok(Redirect::to("/"))
}

pub async fn bootstrap_env_token(state: &AppState) -> anyhow::Result<bool> {
    let Some(access_token) = state.config.strava_access_token.as_ref() else {
        return Ok(false);
    };
    let Some(refresh_token) = state.config.strava_refresh_token.as_ref() else {
        return Ok(false);
    };

    let athlete = match state
        .http
        .get(format!("{STRAVA_API_BASE}/athlete"))
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()
    {
        Ok(response) => response.json::<serde_json::Value>().await?,
        Err(err) if state.config.strava_athlete_id.is_some() => {
            tracing::warn!(
                error = %err,
                "using STRAVA_ATHLETE_ID fallback because /athlete bootstrap failed"
            );
            json!({ "id": state.config.strava_athlete_id.unwrap() })
        }
        Err(err) => return Err(err.into()),
    };

    persist_athlete_and_token(
        state,
        athlete,
        access_token,
        refresh_token,
        state
            .config
            .strava_token_expires_at
            .unwrap_or_else(|| chrono::Utc::now().timestamp() + 3600),
        state
            .config
            .strava_token_scope
            .clone()
            .unwrap_or_else(|| "read,activity:read".to_string()),
    )
    .await?;
    Ok(true)
}

pub async fn webhook_challenge(
    State(state): State<AppState>,
    Query(query): Query<WebhookChallenge>,
) -> Result<Json<serde_json::Value>> {
    if query.mode.as_deref() != Some("subscribe")
        || query.verify_token.as_deref() != Some(&state.config.strava_verify_token)
    {
        return Err(AppError::Unauthorized);
    }
    Ok(Json(
        json!({ "hub.challenge": query.challenge.unwrap_or_default() }),
    ))
}

pub async fn webhook_event(
    State(state): State<AppState>,
    Json(event): Json<WebhookEvent>,
) -> Result<StatusCode> {
    sqlx::query(
        r#"
        INSERT INTO webhook_events
            (object_type, object_id, aspect_type, owner_id, subscription_id, updates_json)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&event.object_type)
    .bind(event.object_id)
    .bind(&event.aspect_type)
    .bind(event.owner_id)
    .bind(event.subscription_id)
    .bind(event.updates.to_string())
    .execute(&state.db)
    .await?;

    sqlx::query("INSERT INTO sync_jobs (job_type, payload_json) VALUES (?, ?)")
        .bind("webhook_event")
        .bind(serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string()))
        .execute(&state.db)
        .await?;

    Ok(StatusCode::OK)
}

pub async fn sync_now(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode> {
    auth::require_user(&state, &headers).await?;
    sqlx::query("INSERT INTO sync_jobs (job_type, payload_json) VALUES (?, ?)")
        .bind("manual_sync")
        .bind(json!({ "backfill_days": state.config.strava_backfill_days }).to_string())
        .execute(&state.db)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn exchange_code(state: &AppState, code: &str, scopes: String) -> Result<()> {
    let client_id = state
        .config
        .strava_client_id
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("STRAVA_CLIENT_ID is not configured".into()))?;
    let client_secret = state
        .config
        .strava_client_secret
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("STRAVA_CLIENT_SECRET is not configured".into()))?;

    let response = state
        .http
        .post("https://www.strava.com/oauth/token")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;

    let athlete = response
        .get("athlete")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Strava token response missing athlete".into()))?;

    persist_athlete_and_token(
        state,
        athlete,
        response
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        response
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        response
            .get("expires_at")
            .and_then(|v| v.as_i64())
            .unwrap_or_default(),
        scopes,
    )
    .await?;

    Ok(())
}

async fn persist_athlete_and_token(
    state: &AppState,
    athlete: serde_json::Value,
    access_token: &str,
    refresh_token: &str,
    expires_at: i64,
    scopes: String,
) -> Result<()> {
    let strava_athlete_id = athlete
        .get("id")
        .and_then(|value| value.as_i64())
        .ok_or_else(|| AppError::BadRequest("Strava athlete response missing id".into()))?;

    let athlete_id: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO athletes
            (strava_athlete_id, username, firstname, lastname, profile_url, raw_profile_json)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(strava_athlete_id) DO UPDATE SET
            username = excluded.username,
            firstname = excluded.firstname,
            lastname = excluded.lastname,
            profile_url = excluded.profile_url,
            raw_profile_json = excluded.raw_profile_json,
            updated_at = CURRENT_TIMESTAMP
        RETURNING id
        "#,
    )
    .bind(strava_athlete_id)
    .bind(athlete.get("username").and_then(|v| v.as_str()))
    .bind(athlete.get("firstname").and_then(|v| v.as_str()))
    .bind(athlete.get("lastname").and_then(|v| v.as_str()))
    .bind(athlete.get("profile").and_then(|v| v.as_str()))
    .bind(athlete.to_string())
    .fetch_one(&state.db)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO strava_tokens (athlete_id, access_token, refresh_token, expires_at, scopes)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(athlete_id) DO UPDATE SET
            access_token = excluded.access_token,
            refresh_token = excluded.refresh_token,
            expires_at = excluded.expires_at,
            scopes = excluded.scopes,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(athlete_id.0)
    .bind(access_token)
    .bind(refresh_token)
    .bind(expires_at)
    .bind(scopes)
    .execute(&state.db)
    .await?;

    Ok(())
}

fn parse_scopes(scopes: &str) -> Vec<String> {
    scopes
        .split([',', ' '])
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_event_deserializes_updates() {
        let event: WebhookEvent = serde_json::from_value(json!({
            "object_type": "activity",
            "object_id": 123,
            "aspect_type": "create",
            "owner_id": 456,
            "subscription_id": 789,
            "updates": { "title": "New title" }
        }))
        .unwrap();

        assert_eq!(event.object_type, "activity");
        assert_eq!(event.updates["title"], "New title");
    }

    #[test]
    fn parses_space_or_comma_delimited_scopes() {
        assert_eq!(
            parse_scopes("read,activity:read activity:read_all"),
            vec!["read", "activity:read", "activity:read_all"]
        );
    }
}

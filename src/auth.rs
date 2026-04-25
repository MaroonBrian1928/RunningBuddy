use crate::{config::Config, error::Result, AppState};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{Duration, Utc};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::error::AppError;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTrainingPlanRequest {
    training_plan: Option<String>,
    training_goals: Option<String>,
    plan_start_date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    authenticated: bool,
    username: Option<String>,
    training_plan: Option<String>,
    training_goals: Option<String>,
    plan_start_date: Option<String>,
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: i64,
    username: String,
    password_hash: String,
    training_plan: Option<String>,
    training_goals: Option<String>,
    plan_start_date: Option<String>,
}

pub async fn ensure_admin_user(pool: &SqlitePool, config: &Config) -> anyhow::Result<()> {
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM app_users WHERE username = ?")
        .bind(&config.admin_username)
        .fetch_optional(pool)
        .await?;

    if existing.is_none() {
        let password_hash = hash_password(&config.admin_password)?;
        sqlx::query("INSERT INTO app_users (username, password_hash) VALUES (?, ?)")
            .bind(&config.admin_username)
            .bind(password_hash)
            .execute(pool)
            .await?;
        tracing::info!(username = %config.admin_username, "created admin user");
    }

    Ok(())
}

pub async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse> {
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, training_plan, training_goals, plan_start_date FROM app_users WHERE username = ?",
    )
    .bind(&payload.username)
    .fetch_optional(&state.db)
    .await?;

    let Some(user) = user else {
        return Err(AppError::Unauthorized);
    };
    verify_password(&payload.password, &user.password_hash)?;

    let session_id = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(30);
    sqlx::query("INSERT INTO app_sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&session_id)
        .bind(user.id)
        .bind(expires_at.to_rfc3339())
        .execute(&state.db)
        .await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
            state.config.session_cookie,
            session_id,
            Duration::days(30).num_seconds()
        ))
        .map_err(|err| AppError::BadRequest(err.to_string()))?,
    );

    Ok((
        headers,
        Json(MeResponse {
            authenticated: true,
            username: Some(user.username),
            training_plan: user.training_plan,
            training_goals: user.training_goals,
            plan_start_date: user.plan_start_date,
        }),
    ))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    if let Some(session_id) = session_from_headers(&headers, &state.config.session_cookie) {
        sqlx::query("DELETE FROM app_sessions WHERE id = ?")
            .bind(session_id)
            .execute(&state.db)
            .await?;
    }

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0",
            state.config.session_cookie
        ))
        .map_err(|err| AppError::BadRequest(err.to_string()))?,
    );

    Ok((StatusCode::NO_CONTENT, response_headers))
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<MeResponse>> {
    let user = current_user(&state, &headers).await.ok();
    Ok(Json(MeResponse {
        authenticated: user.is_some(),
        username: user.as_ref().map(|user| user.username.clone()),
        training_plan: user.as_ref().and_then(|user| user.training_plan.clone()),
        training_goals: user.as_ref().and_then(|user| user.training_goals.clone()),
        plan_start_date: user.as_ref().and_then(|user| user.plan_start_date.clone()),
    }))
}

pub async fn update_training_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateTrainingPlanRequest>,
) -> Result<impl IntoResponse> {
    let user = require_user(&state, &headers).await?;
    sqlx::query(
        "UPDATE app_users SET training_plan = ?, training_goals = ?, plan_start_date = ? WHERE id = ?",
    )
        .bind(clean_optional(payload.training_plan))
        .bind(clean_optional(payload.training_goals))
        .bind(clean_optional(payload.plan_start_date))
        .bind(user.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

pub async fn require_user(state: &AppState, headers: &HeaderMap) -> Result<UserIdentity> {
    current_user(state, headers).await
}

async fn current_user(state: &AppState, headers: &HeaderMap) -> Result<UserIdentity> {
    let session_id = session_from_headers(headers, &state.config.session_cookie)
        .ok_or(AppError::Unauthorized)?;
    let user = sqlx::query_as::<_, UserIdentity>(
        r#"
        SELECT u.id, u.username, u.training_plan, u.training_goals, u.plan_start_date
        FROM app_sessions s
        JOIN app_users u ON u.id = s.user_id
        WHERE s.id = ? AND s.expires_at > datetime('now')
        "#,
    )
    .bind(session_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;
    Ok(user)
}

fn session_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == cookie_name).then(|| value.to_string())
    })
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!("failed to hash password: {err}"))?
        .to_string())
}

fn verify_password(password: &str, password_hash: &str) -> Result<()> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| AppError::Unauthorized)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AppError::Unauthorized)
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserIdentity {
    pub id: i64,
    pub username: String,
    pub training_plan: Option<String>,
    pub training_goals: Option<String>,
    pub plan_start_date: Option<String>,
}

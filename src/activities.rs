use crate::{
    auth,
    error::{AppError, Result},
    models::{ActivityDetail, ActivitySummary},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ActivitySummary>>> {
    auth::require_user(&state, &headers).await?;
    let activities = sqlx::query_as::<_, ActivitySummary>(
        r#"
        SELECT id, strava_activity_id, name, sport_type, start_date, moving_time_seconds,
               distance_meters, average_heartrate, total_elevation_gain, deleted_at,
               private_unavailable
        FROM activities
        ORDER BY start_date DESC, id DESC
        LIMIT 200
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(activities))
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<ActivityDetail>> {
    auth::require_user(&state, &headers).await?;
    let activity = sqlx::query_as::<_, ActivityDetail>(
        r#"
        SELECT id, strava_activity_id, name, sport_type, start_date, elapsed_time_seconds,
               moving_time_seconds, distance_meters, total_elevation_gain, average_heartrate,
               max_heartrate, average_speed, max_speed, average_cadence, average_watts,
               kilojoules, suffer_score, visibility, deleted_at, private_unavailable,
               raw_activity_json
        FROM activities
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(activity))
}

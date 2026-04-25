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
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct GenerateAdviceRequest {
    #[serde(default = "default_window")]
    input_window_days: i64,
}

#[derive(Debug, Serialize)]
struct AdvicePromptActivity {
    name: String,
    sport_type: Option<String>,
    start_date: Option<String>,
    moving_time_seconds: Option<i64>,
    distance_meters: Option<f64>,
    average_heartrate: Option<f64>,
}

pub async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GenerateAdviceRequest>,
) -> Result<Json<TrainingAdviceResponse>> {
    auth::require_user(&state, &headers).await?;
    let activities = recent_activities(&state, payload.input_window_days).await?;
    let body = request_advice(&state, payload.input_window_days, &activities).await?;
    let response = persist_advice(&state, payload.input_window_days, &body).await?;
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

async fn recent_activities(state: &AppState, days: i64) -> Result<Vec<AdvicePromptActivity>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<f64>,
            Option<f64>,
        ),
    >(
        r#"
        SELECT name, sport_type, start_date, moving_time_seconds, distance_meters, average_heartrate
        FROM activities
        WHERE deleted_at IS NULL
          AND private_unavailable = 0
          AND (start_date IS NULL OR start_date >= datetime('now', ?))
        ORDER BY start_date DESC
        LIMIT 100
        "#,
    )
    .bind(format!("-{days} days"))
    .fetch_all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| AdvicePromptActivity {
            name: row.0,
            sport_type: row.1,
            start_date: row.2,
            moving_time_seconds: row.3,
            distance_meters: row.4,
            average_heartrate: row.5,
        })
        .collect())
}

async fn request_advice(
    state: &AppState,
    input_window_days: i64,
    activities: &[AdvicePromptActivity],
) -> Result<TrainingAdviceBody> {
    if activities.is_empty() {
        return Ok(local_fallback_advice(input_window_days));
    }

    match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_advice(state, input_window_days, activities).await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_advice(state, input_window_days, activities).await
        }
        _ => Ok(local_fallback_advice(input_window_days)),
    }
}

async fn openai_advice(
    state: &AppState,
    input_window_days: i64,
    activities: &[AdvicePromptActivity],
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let payload = json!({
        "model": state.config.llm_model,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": advice_system_prompt() },
            { "role": "user", "content": json!({
                "input_window_days": input_window_days,
                "activities": activities,
            }).to_string() }
        ]
    });
    let response = state
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
        .json::<serde_json::Value>()
        .await?;
    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AppError::BadRequest("OpenAI response did not include JSON content".into())
        })?;
    parse_advice_body(content)
}

async fn gemini_advice(
    state: &AppState,
    input_window_days: i64,
    activities: &[AdvicePromptActivity],
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!("{}\n\n{}", advice_system_prompt(), json!({
                "input_window_days": input_window_days,
                "activities": activities,
            })) }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json"
        }
    });
    let response = state
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
        .json::<serde_json::Value>()
        .await?;
    let content = response["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            AppError::BadRequest("Gemini response did not include JSON content".into())
        })?;
    parse_advice_body(content)
}

fn parse_advice_body(content: &str) -> Result<TrainingAdviceBody> {
    serde_json::from_str(content).map_err(|err| AppError::BadRequest(err.to_string()))
}

async fn persist_advice(
    state: &AppState,
    input_window_days: i64,
    body: &TrainingAdviceBody,
) -> Result<TrainingAdviceResponse> {
    let row = sqlx::query_as::<_, TrainingAdviceRow>(
        r#"
        INSERT INTO training_advice
            (provider, model, input_window_days, summary, load_observations_json,
             risks_json, next_7_days_json, recovery_notes, confidence, safety_note,
             raw_response_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(&state.config.llm_provider)
    .bind(&state.config.llm_model)
    .bind(input_window_days)
    .bind(&body.summary)
    .bind(serde_json::to_string(&body.load_observations).unwrap())
    .bind(serde_json::to_string(&body.risks).unwrap())
    .bind(serde_json::to_string(&body.next_7_days).unwrap())
    .bind(&body.recovery_notes)
    .bind(body.confidence)
    .bind(&body.safety_note)
    .bind(serde_json::to_string(body).unwrap())
    .fetch_one(&state.db)
    .await?;
    Ok(row.into_response())
}

fn local_fallback_advice(input_window_days: i64) -> TrainingAdviceBody {
    TrainingAdviceBody {
        summary: format!(
            "No configured LLM response is available for the last {input_window_days} days yet."
        ),
        load_observations: vec![
            "Connect Strava and sync activities to build a useful training load picture.".into(),
        ],
        risks: vec![
            "Avoid ramping volume or intensity sharply while the app has limited history.".into(),
        ],
        next_7_days: vec![
            "Keep most running easy until recent activity history is synced.".into(),
            "Add one rest or mobility-focused day if soreness is elevated.".into(),
        ],
        recovery_notes: "Prioritize sleep, hydration, and easy aerobic consistency.".into(),
        confidence: 0.2,
        safety_note: "This is general training guidance, not medical advice or injury treatment."
            .into(),
    }
}

fn advice_system_prompt() -> &'static str {
    "Return strict JSON with summary, load_observations, risks, next_7_days, recovery_notes, confidence, and safety_note. Advice must be non-medical and conservative."
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
                "confidence":0.7,
                "safety_note":"not medical advice"
            }"#,
        )
        .unwrap();

        assert_eq!(body.summary, "steady");
        assert_eq!(body.next_7_days.len(), 1);
    }
}

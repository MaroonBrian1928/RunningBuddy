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

pub async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GenerateAdviceRequest>,
) -> Result<Json<TrainingAdviceResponse>> {
    let user = auth::require_user(&state, &headers).await?;
    let activities = recent_activities(&state, payload.input_window_days).await?;
    let target_activity = match payload.activity_id {
        Some(activity_id) => Some(activity(&state, activity_id).await?),
        None => None,
    };
    let profile = TrainingProfile {
        plan: user.training_plan.as_deref(),
        goals: user.training_goals.as_deref(),
        plan_start_date: user.plan_start_date.as_deref(),
    };
    let body = request_advice(
        &state,
        payload.input_window_days,
        &activities,
        target_activity.as_ref(),
        &profile,
    )
    .await?;
    let response = persist_advice(
        &state,
        payload.input_window_days,
        payload.activity_id,
        &body,
    )
    .await?;
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
    auth::require_user(&state, &headers).await?;
    let row = sqlx::query_as::<_, TrainingAdviceRow>("SELECT * FROM training_advice WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let advice = row.into_response();
    let message = request_advice_chat(&state, &advice.body, &payload.messages).await?;
    Ok(Json(AdviceChatResponse { message }))
}

async fn recent_activities(state: &AppState, days: i64) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT raw_activity_json
        FROM activities
        WHERE deleted_at IS NULL
          AND private_unavailable = 0
          AND sport_type = 'Run'
          AND (start_date IS NULL OR start_date >= datetime('now', ?))
        ORDER BY start_date DESC
        LIMIT 100
        "#,
    )
    .bind(format!("-{days} days"))
    .fetch_all(&state.db)
    .await?;

    let mut activities = Vec::new();
    for row in rows {
        if let Ok(json) = serde_json::from_str(&row.0) {
            activities.push(json);
        }
    }
    Ok(activities)
}

async fn activity(state: &AppState, activity_id: i64) -> Result<serde_json::Value> {
    let row = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT raw_activity_json
        FROM activities
        WHERE id = ?
          AND deleted_at IS NULL
          AND private_unavailable = 0
        "#,
    )
    .bind(activity_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    serde_json::from_str(&row.0).map_err(|err| AppError::BadRequest(err.to_string()))
}

async fn request_advice(
    state: &AppState,
    input_window_days: i64,
    activities: &[serde_json::Value],
    target_activity: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    if activities.is_empty() {
        tracing::info!("no activities available, using local fallback advice");
        return Ok(local_fallback_advice(
            input_window_days,
            target_activity.is_some(),
        ));
    }

    tracing::info!(
        provider = state.config.llm_provider,
        model = state.config.llm_model,
        activities_count = activities.len(),
        has_target_activity = target_activity.is_some(),
        has_training_plan = profile.plan.is_some(),
        has_training_goals = profile.goals.is_some(),
        "requesting training advice"
    );

    match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_advice(
                state,
                input_window_days,
                activities,
                target_activity,
                profile,
            )
            .await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_advice(
                state,
                input_window_days,
                activities,
                target_activity,
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
    activities: &[serde_json::Value],
    target_activity: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let mut user_content = json!({
        "advice_request_scope": advice_request_scope(target_activity),
        "input_window_days": input_window_days,
        "activities": activities,
        "training_profile": {
            "plan": profile.plan,
            "goals": profile.goals,
            "plan_start_date": profile.plan_start_date,
        },
    });
    if let Some(activity) = target_activity {
        user_content["target_activity"] = activity.clone();
    }

    let payload = json!({
        "model": state.config.llm_model,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": advice_system_prompt() },
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
    activities: &[serde_json::Value],
    target_activity: Option<&serde_json::Value>,
    profile: &TrainingProfile<'_>,
) -> Result<TrainingAdviceBody> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let mut user_content = json!({
        "advice_request_scope": advice_request_scope(target_activity),
        "input_window_days": input_window_days,
        "activities": activities,
        "training_profile": {
            "plan": profile.plan,
            "goals": profile.goals,
            "plan_start_date": profile.plan_start_date,
        },
    });
    if let Some(activity) = target_activity {
        user_content["target_activity"] = activity.clone();
    }

    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!("{}\n\n{}", advice_system_prompt(), user_content) }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json"
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
) -> Result<String> {
    match state.config.llm_provider.as_str() {
        "openai" if state.config.openai_api_key.is_some() => {
            openai_advice_chat(state, advice, messages).await
        }
        "gemini" if state.config.gemini_api_key.is_some() => {
            gemini_advice_chat(state, advice, messages).await
        }
        _ => Ok(local_chat_fallback(advice, messages)),
    }
}

async fn openai_advice_chat(
    state: &AppState,
    advice: &TrainingAdviceBody,
    messages: &[AdviceChatMessage],
) -> Result<String> {
    let api_key = state.config.openai_api_key.as_ref().unwrap();
    let chat_messages = json!([
        { "role": "system", "content": advice_chat_system_prompt() },
        { "role": "user", "content": json!({
            "saved_advice": advice_chat_context(advice),
            "conversation": messages,
        }).to_string() }
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
) -> Result<String> {
    let api_key = state.config.gemini_api_key.as_ref().unwrap();
    let payload = json!({
        "contents": [{
            "parts": [{ "text": format!(
                "{}\n\n{}",
                advice_chat_system_prompt(),
                json!({
                    "saved_advice": advice_chat_context(advice),
                    "conversation": messages,
                })
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
             risks_json, next_7_days_json, recovery_notes, confidence, safety_note,
             raw_response_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
    .bind(&body.safety_note)
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
        safety_note: String::new(),
    }
}

fn advice_request_scope(target_activity: Option<&serde_json::Value>) -> &'static str {
    if target_activity.is_some() {
        "activity_review"
    } else {
        "training_overview"
    }
}

fn advice_system_prompt() -> &'static str {
    r#"You are acting as an experienced running coach focused on practical, evidence-based training guidance.

Your job is to help me successfully complete a specific running plan I provide. Your role is not to blindly repeat the plan, but to interpret it, explain it, adapt it when needed, and help me execute it consistently and safely.

Your coaching style should be:

* Direct, precise, and honest
* Focused on training outcomes, recovery, injury prevention, and long-term consistency
* Skeptical of vague assumptions and quick fixes
* Willing to challenge poor decisions (skipping recovery, running too hard too often, unrealistic pacing, etc.)
* Structured and specific rather than motivational fluff

When responding:

* First understand the full training plan, goal race/event, timeline, current fitness level, injury history, available training days, and constraints (work, family, travel, equipment, terrain, weather)
* Help translate workouts into actionable pacing, effort, heart rate, or RPE guidance
* Explain the purpose of each workout (easy run, tempo, intervals, long run, recovery, deload, etc.)
* Identify if the plan is too aggressive, too conservative, or internally inconsistent
* Suggest modifications only when justified, and explain why
* Prioritize consistency over hero workouts
* Consider sleep, nutrition, hydration, fueling, strength work, and recovery as part of the plan
* Flag overtraining risk, poor progression, or likely injury traps early
* If information is missing, ask targeted follow-up questions instead of assuming

Do not:

* Give generic "listen to your body" advice without specifics
* Default to encouragement over accuracy
* Assume every plan is well-designed
* Recommend increasing intensity without strong justification

Return strict JSON matching this structure exactly: { "summary": string, "load_observations": string[], "risks": string[], "next_7_days": string[], "recovery_notes": string, "confidence": number }.
Write the JSON values in a conversational coaching voice, as if speaking directly to the runner.
Use advice_request_scope, training_profile, activities, and target_activity when provided.

When advice_request_scope is "activity_review":
* Make target_activity the center of the answer. The advice should read like a review of that exact run, not a general training-plan check-in.
* Use the training plan, goals, plan start date, and recent activities only as context for judging whether this activity fit the intended plan, load progression, recovery needs, or next workout.
* Tie every observation, risk, and next-7-day suggestion back to evidence from target_activity when possible: distance, duration, pacing/speed, heart rate, elevation, cadence, sport type, start date, relative effort, and how it compares with recent activities.
* Avoid broad plan feedback unless it directly explains what this activity means or how the runner should adjust after it.
* If target_activity lacks key data, say what is missing and give the narrowest useful activity-specific guidance rather than expanding into generic plan advice.

When advice_request_scope is "training_overview":
* Review the recent activity set and plan as a whole.
* Keep the guidance focused on load, progression, risk, and the next week.
If the training plan, goal race, timeline, current fitness, injury history, available days, or constraints are missing, include concise targeted questions inside the relevant JSON fields instead of inventing details."#
}

fn advice_chat_system_prompt() -> &'static str {
    "You are a running coach continuing a conversation about previously generated training advice. Answer conversationally in plain text, not JSON. Be specific, concise, and practical. Use the saved_advice as the source of truth, answer the latest user follow-up, and ask one targeted question when key information is missing. Do not add repeated safety disclaimers; the app displays one shared footer disclaimer."
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
}

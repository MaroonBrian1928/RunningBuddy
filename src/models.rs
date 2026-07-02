use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Serialize, FromRow)]
pub struct ActivitySummary {
    pub id: i64,
    pub strava_activity_id: i64,
    pub name: String,
    pub sport_type: Option<String>,
    pub start_date: Option<String>,
    pub moving_time_seconds: Option<i64>,
    pub distance_meters: Option<f64>,
    pub average_heartrate: Option<f64>,
    pub total_elevation_gain: Option<f64>,
    pub deleted_at: Option<String>,
    pub private_unavailable: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ActivityDetail {
    pub id: i64,
    pub strava_activity_id: i64,
    pub name: String,
    pub sport_type: Option<String>,
    pub start_date: Option<String>,
    pub elapsed_time_seconds: Option<i64>,
    pub moving_time_seconds: Option<i64>,
    pub distance_meters: Option<f64>,
    pub total_elevation_gain: Option<f64>,
    pub average_heartrate: Option<f64>,
    pub max_heartrate: Option<f64>,
    pub average_speed: Option<f64>,
    pub max_speed: Option<f64>,
    pub average_cadence: Option<f64>,
    pub average_watts: Option<f64>,
    pub kilojoules: Option<f64>,
    pub suffer_score: Option<f64>,
    pub visibility: Option<String>,
    pub deleted_at: Option<String>,
    pub private_unavailable: i64,
    pub raw_activity_json: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrainingAdviceBody {
    pub summary: String,
    pub load_observations: Vec<String>,
    pub risks: Vec<String>,
    pub next_7_days: Vec<String>,
    pub recovery_notes: String,
    pub confidence: f64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct TrainingAdviceRow {
    pub id: i64,
    pub activity_id: Option<i64>,
    pub provider: String,
    pub model: String,
    pub input_window_days: i64,
    pub summary: String,
    pub load_observations_json: String,
    pub risks_json: String,
    pub next_7_days_json: String,
    pub recovery_notes: String,
    pub confidence: f64,
    pub raw_response_json: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct TrainingAdviceResponse {
    pub id: i64,
    pub activity_id: Option<i64>,
    pub provider: String,
    pub model: String,
    pub input_window_days: i64,
    pub body: TrainingAdviceBody,
    pub created_at: String,
}

impl TrainingAdviceRow {
    pub fn into_response(self) -> TrainingAdviceResponse {
        TrainingAdviceResponse {
            id: self.id,
            activity_id: self.activity_id,
            provider: self.provider,
            model: self.model,
            input_window_days: self.input_window_days,
            body: TrainingAdviceBody {
                summary: self.summary,
                load_observations: serde_json::from_str(&self.load_observations_json)
                    .unwrap_or_default(),
                risks: serde_json::from_str(&self.risks_json).unwrap_or_default(),
                next_7_days: serde_json::from_str(&self.next_7_days_json).unwrap_or_default(),
                recovery_notes: self.recovery_notes,
                confidence: self.confidence,
            },
            created_at: self.created_at,
        }
    }
}

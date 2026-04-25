pub mod activities;
pub mod advice;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod strava;
pub mod sync;

use axum::{
    routing::{get, post},
    Router,
};
use reqwest::Client;
use sqlx::SqlitePool;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    pub config: config::Config,
    pub db: SqlitePool,
    pub http: Client,
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route(
            "/api/auth/training-plan",
            axum::routing::put(auth::update_training_plan),
        )
        .route("/api/strava/connect", get(strava::connect))
        .route("/api/strava/callback", get(strava::callback))
        .route("/api/strava/status", get(strava::status))
        .route("/api/strava/sync", post(strava::sync_now))
        .route("/strava/webhook", get(strava::webhook_challenge))
        .route("/strava/webhook", post(strava::webhook_event))
        .route("/api/activities", get(activities::list))
        .route("/api/activities/{id}", get(activities::detail))
        .route("/api/advice", post(advice::generate))
        .route("/api/advice", get(advice::list))
        .route("/api/advice/{id}", get(advice::detail))
        .fallback_service(ServeDir::new("frontend/dist").append_index_html_on_directories(true))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn router_builds_without_route_syntax_panics() {
        let db = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("in-memory sqlite pool");
        let state = AppState {
            config: config::Config::from_env(),
            db,
            http: Client::new(),
        };

        let _ = app(state);
    }
}

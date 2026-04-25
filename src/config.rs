use std::{env, net::SocketAddr};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub admin_username: String,
    pub admin_password: String,
    pub session_cookie: String,
    pub public_url: String,
    pub strava_client_id: Option<String>,
    pub strava_client_secret: Option<String>,
    pub strava_athlete_id: Option<i64>,
    pub strava_access_token: Option<String>,
    pub strava_refresh_token: Option<String>,
    pub strava_token_expires_at: Option<i64>,
    pub strava_token_scope: Option<String>,
    pub strava_verify_token: String,
    pub strava_backfill_days: i64,
    pub llm_provider: String,
    pub llm_model: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    pub gemini_api_key: Option<String>,
    pub gemini_base_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: env_or("RUNNINGBUDDY_DATABASE_URL", "sqlite://runningbuddy.db"),
            bind_addr: env_or("RUNNINGBUDDY_BIND_ADDR", "127.0.0.1:3000")
                .parse()
                .expect("RUNNINGBUDDY_BIND_ADDR must be host:port"),
            admin_username: env_or("RUNNINGBUDDY_ADMIN_USERNAME", "admin"),
            admin_password: env_or("RUNNINGBUDDY_ADMIN_PASSWORD", "change-me"),
            session_cookie: env_or("RUNNINGBUDDY_SESSION_COOKIE", "runningbuddy_session"),
            public_url: env_or("RUNNINGBUDDY_PUBLIC_URL", "http://127.0.0.1:3000"),
            strava_client_id: nonempty_env("STRAVA_CLIENT_ID"),
            strava_client_secret: nonempty_env("STRAVA_CLIENT_SECRET"),
            strava_athlete_id: nonempty_env("STRAVA_ATHLETE_ID")
                .and_then(|value| value.parse().ok()),
            strava_access_token: nonempty_env("STRAVA_ACCESS_TOKEN"),
            strava_refresh_token: nonempty_env("STRAVA_REFRESH_TOKEN"),
            strava_token_expires_at: nonempty_env("STRAVA_TOKEN_EXPIRES_AT")
                .and_then(|value| value.parse().ok()),
            strava_token_scope: nonempty_env("STRAVA_TOKEN_SCOPE"),
            strava_verify_token: env_or("STRAVA_VERIFY_TOKEN", "change-me"),
            strava_backfill_days: env_or("STRAVA_BACKFILL_DAYS", "180").parse().unwrap_or(180),
            llm_provider: env_or("LLM_PROVIDER", "openai"),
            llm_model: env_or("LLM_MODEL", "gpt-4.1-mini"),
            openai_api_key: nonempty_env("OPENAI_API_KEY"),
            openai_base_url: env_or("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            gemini_api_key: nonempty_env("GEMINI_API_KEY"),
            gemini_base_url: env_or(
                "GEMINI_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta",
            ),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn nonempty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

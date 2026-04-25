use anyhow::Context;
use runningbuddy::{app, auth, config::Config, db, strava, sync, AppState};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    let pool = db::connect(&config.database_url)
        .await
        .context("connect to sqlite")?;
    db::migrate(&pool).await.context("run migrations")?;
    auth::ensure_admin_user(&pool, &config).await?;

    let state = AppState {
        config: config.clone(),
        db: pool,
        http: reqwest::Client::new(),
    };
    if let Err(err) = strava::bootstrap_env_token(&state).await {
        tracing::warn!(error = %err, "failed to bootstrap Strava token from environment");
    }
    sync::spawn_worker(state.clone());
    let listener = TcpListener::bind(&config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "starting RunningBuddy API");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

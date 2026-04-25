use anyhow::Context;
use sqlx::{migrate::Migrator, sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;

pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .with_context(|| format!("failed to connect to {database_url}"))
}

pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let migrator = Migrator::new(Path::new("./migrations")).await?;
    migrator.run(pool).await?;
    Ok(())
}

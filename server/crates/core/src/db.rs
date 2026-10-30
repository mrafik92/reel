//! SQLite pool (WAL, foreign keys) and migrations.

use std::{path::Path, str::FromStr, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::Result;

/// Open (creating if needed) a file-backed pool and run migrations.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new().connect_with(opts).await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Open from a URL such as `sqlite::memory:` (single connection) and migrate.
pub async fn connect_url(url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?.foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Apply embedded migrations.
pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_in_memory() {
        let pool = connect_url("sqlite::memory:").await.unwrap();
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('users','media_items','requests','epg_programs')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 4);
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }
}

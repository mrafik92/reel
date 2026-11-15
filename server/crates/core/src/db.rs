//! SQLite pool (WAL, foreign keys) and migrations.

use std::{path::Path, str::FromStr, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::Result;

/// Suffixes of the files SQLite keeps next to the database.
const SIDE_FILES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Open (creating if needed) a file-backed pool and run migrations.
///
/// The database and its side files are owner-only (`0600`): a new database is created empty with that mode
/// before SQLite opens it, and SQLite gives `-wal`/`-shm` the mode of the database file. Looser modes left
/// by older builds are repaired here (#14).
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        crate::fsperm::create_private_dir_all(dir)?;
    }
    crate::fsperm::create_private_file(path)?;
    restrict_db_files(path);
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new().connect_with(opts).await?;
    migrate(&pool).await?;
    // SQLite copies the database's mode to new side files; re-check in case the database couldn't be fixed.
    restrict_db_files(path);
    Ok(pool)
}

fn restrict_db_files(path: &Path) {
    crate::fsperm::repair(path);
    for suffix in SIDE_FILES {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        crate::fsperm::repair(Path::new(&side));
    }
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

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn new_database_and_wal_files_are_owner_only() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("data/sub/reel.db");
        let pool = connect(&db).await.unwrap();
        sqlx::query("CREATE TABLE t (x)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(mode(&t.path().join("data")), 0o700);
        assert_eq!(mode(&t.path().join("data/sub")), 0o700);
        for f in ["reel.db", "reel.db-wal", "reel.db-shm"] {
            let p = t.path().join("data/sub").join(f);
            assert!(p.exists(), "{f} missing");
            assert_eq!(mode(&p), 0o600, "{f}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn loose_database_files_are_repaired_on_open() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("reel.db");
        // a database written by an older build: 0644 with WAL side files
        let pool = connect(&db).await.unwrap();
        sqlx::query("CREATE TABLE t (x)")
            .execute(&pool)
            .await
            .unwrap();
        for f in ["reel.db", "reel.db-wal", "reel.db-shm"] {
            let p = t.path().join(f);
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        drop(pool);
        let pool = connect(&db).await.unwrap();
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        for f in ["reel.db", "reel.db-wal", "reel.db-shm"] {
            assert_eq!(mode(&t.path().join(f)), 0o600, "{f}");
        }
    }

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

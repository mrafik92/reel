mod common;

use std::time::Duration;

use common::*;
use reel_core::db;
use reel_scanner::Scanner;
use sqlx::SqlitePool;

async fn item_id(pool: &SqlitePool, title: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM media_items WHERE title = ? AND kind IN ('movie','episode')")
        .bind(title)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn setup() -> (tempfile::TempDir, std::path::PathBuf, SqlitePool, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let movies = tmp.path().join("movies");
    make_video(&movies.join("Alpha (2020)/Alpha (2020).mkv"), 1);
    make_video(&movies.join("Beta (2021)/Beta (2021).mkv"), 2);
    let pool = db::connect_url("sqlite::memory:").await.unwrap();
    let lib = add_library(&pool, "movies", &movies).await;
    (tmp, movies, pool, lib)
}

#[tokio::test]
async fn rename_in_a_scan_keeps_item_metadata_and_watch_state() {
    if skip() {
        return;
    }
    let (_t, movies, pool, lib) = setup().await;
    let scanner = Scanner::new();
    scanner.scan_library(&pool, lib).await.unwrap();
    let alpha = item_id(&pool, "Alpha").await;
    sqlx::query("UPDATE media_items SET overview = 'Curated plot', tmdb_id = 42 WHERE id = ?")
        .bind(alpha)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO watch_state (user_id, item_id, position_s, played, play_count) VALUES (1, ?, 0.5, 1, 3)")
        .bind(alpha)
        .execute(&pool)
        .await
        .unwrap();
    let file_id = count(
        &pool,
        &format!("SELECT id FROM media_files WHERE item_id = {alpha}"),
    )
    .await;

    std::fs::rename(
        movies.join("Alpha (2020)/Alpha (2020).mkv"),
        movies.join("Alpha (2020)/Alpha 2020 1080p.mkv"),
    )
    .unwrap();
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!(
        (s.moved, s.added, s.removed, s.updated),
        (1, 0, 0, 0),
        "{s:?}"
    );

    assert_eq!(item_id(&pool, "Alpha").await, alpha, "same item");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_items WHERE kind='movie'").await,
        2
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM media_files").await, 2);
    let (fid, path): (i64, String) =
        sqlx::query_as("SELECT id, path FROM media_files WHERE item_id = ?")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fid, file_id, "same file row, re-pointed");
    assert!(path.ends_with("Alpha 2020 1080p.mkv"), "{path}");
    let (ov, tm): (String, i64) =
        sqlx::query_as("SELECT overview, tmdb_id FROM media_items WHERE id = ?")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((ov.as_str(), tm), ("Curated plot", 42));
    let (pos, played, pc): (f64, i64, i64) =
        sqlx::query_as("SELECT position_s, played, play_count FROM watch_state WHERE item_id = ?")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((pos, played, pc), (0.5, 1, 3));
    assert_eq!(
        count(
            &pool,
            &format!("SELECT count(*) FROM streams WHERE file_id = {fid}")
        )
        .await,
        2,
        "streams refreshed"
    );
    // a further scan is a no-op
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.moved, s.added, s.removed, s.unchanged), (0, 0, 0, 2));
}

#[tokio::test]
async fn renaming_a_whole_show_folder_keeps_every_episode() {
    if skip() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let shows = tmp.path().join("shows");
    for (n, secs) in [(1, 1), (2, 2), (3, 3)] {
        make_video(
            &shows.join(format!(
                "Some Show (2019)/Season 01/Some Show - S01E0{n}.mkv"
            )),
            secs,
        );
    }
    let pool = db::connect_url("sqlite::memory:").await.unwrap();
    let lib = add_library(&pool, "shows", &shows).await;
    let scanner = Scanner::new();
    scanner.scan_library(&pool, lib).await.unwrap();
    let before: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(before.len(), 3);
    sqlx::query(
        "INSERT INTO watch_state (user_id, item_id, position_s, played) VALUES (1, ?, 0, 1)",
    )
    .bind(before[1].0)
    .execute(&pool)
    .await
    .unwrap();

    // move the episodes into a differently named folder structure
    std::fs::rename(
        shows.join("Some Show (2019)"),
        shows.join("Some Show 2019 [WEB-DL]"),
    )
    .unwrap();
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.moved, s.added, s.removed), (3, 0, 0), "{s:?}");
    let after: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(before, after, "episode ids preserved");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_items WHERE kind='show'").await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM watch_state WHERE played = 1").await,
        1
    );
}

#[tokio::test]
async fn rename_across_separate_events_works_in_either_order() {
    if skip() {
        return;
    }
    for old_first in [true, false] {
        let (_t, movies, pool, lib) = setup().await;
        let scanner = Scanner::new().with_rename_window(Duration::from_secs(30));
        scanner.scan_library(&pool, lib).await.unwrap();
        let alpha = item_id(&pool, "Alpha").await;
        let (old, new) = (
            movies.join("Alpha (2020)/Alpha (2020).mkv"),
            movies.join("Moved/Alpha (2020) remux.mkv"),
        );
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::rename(&old, &new).unwrap();

        // a watcher delivers the two paths as separate targeted rescans
        let (first, second) = if old_first {
            (&old, &new)
        } else {
            (&new, &old)
        };
        let a = scanner.scan_path(&pool, first).await.unwrap();
        let b = scanner.scan_path(&pool, second).await.unwrap();
        let (moved, added) = (a.moved + b.moved, a.added + b.added);
        assert_eq!((moved, added), (1, 0), "old_first={old_first}: {a:?} {b:?}");
        assert_eq!(
            item_id(&pool, "Alpha").await,
            alpha,
            "old_first={old_first}"
        );
        assert_eq!(
            count(&pool, "SELECT count(*) FROM media_items WHERE kind='movie'").await,
            2
        );
        let path: String = sqlx::query_scalar("SELECT path FROM media_files WHERE item_id = ?")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(path.ends_with("Alpha (2020) remux.mkv"));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn pending_rename_expires_and_unrelated_files_are_not_claimed() {
    if skip() {
        return;
    }
    // window expired: the old item is dropped and the file shows up as a new item
    let (_t, movies, pool, lib) = setup().await;
    let scanner = Scanner::new().with_rename_window(Duration::from_millis(100));
    scanner.scan_library(&pool, lib).await.unwrap();
    let alpha = item_id(&pool, "Alpha").await;
    let (old, new) = (
        movies.join("Alpha (2020)/Alpha (2020).mkv"),
        movies.join("Elsewhere/Alpha Renamed (2020).mkv"),
    );
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::rename(&old, &new).unwrap();
    let a = scanner.scan_path(&pool, &old).await.unwrap();
    assert_eq!(a.removed, 1);
    assert_eq!(
        count(
            &pool,
            &format!("SELECT count(*) FROM media_items WHERE id = {alpha}")
        )
        .await,
        1,
        "kept during the window"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        count(
            &pool,
            &format!("SELECT count(*) FROM media_items WHERE id = {alpha}")
        )
        .await,
        0,
        "orphan removed afterwards"
    );
    let b = scanner.scan_path(&pool, &new).await.unwrap();
    assert_eq!((b.moved, b.added), (0, 1));
    assert_ne!(item_id(&pool, "Alpha Renamed").await, alpha);

    // different content (other duration) is never treated as a rename
    let (_t, movies, pool, lib) = setup().await;
    let scanner = Scanner::new();
    scanner.scan_library(&pool, lib).await.unwrap();
    let alpha = item_id(&pool, "Alpha").await;
    std::fs::remove_file(movies.join("Alpha (2020)/Alpha (2020).mkv")).unwrap();
    make_video(&movies.join("Gamma (2022)/Gamma (2022).mkv"), 3);
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.moved, s.added, s.removed), (0, 1, 1), "{s:?}");
    assert_eq!(
        count(
            &pool,
            &format!("SELECT count(*) FROM media_items WHERE id = {alpha}")
        )
        .await,
        0
    );
}

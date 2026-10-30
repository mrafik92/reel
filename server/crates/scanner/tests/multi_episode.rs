mod common;

use common::*;
use reel_core::db;
use reel_scanner::Scanner;

#[tokio::test]
async fn multi_episode_file_creates_every_episode_and_links_one_file() {
    if skip() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let shows = tmp.path().join("shows");
    let double = shows.join("Some Show (2019)/Season 01/Some Show - S01E02E03.mkv");
    make_video(
        &shows.join("Some Show (2019)/Season 01/Some Show - S01E01.mkv"),
        1,
    );
    make_video(&double, 2);
    // a range form, 3 episodes in one file
    make_video(
        &shows.join("Some Show (2019)/Season 01/Some Show - S01E04-E06.mkv"),
        3,
    );

    let pool = db::connect_url("sqlite::memory:").await.unwrap();
    let lib = add_library(&pool, "shows", &shows).await;
    let scanner = Scanner::new();
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.added, s.failed), (3, 0));

    let eps: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let nos: Vec<i64> = eps.iter().map(|e| e.1).collect();
    assert_eq!(
        nos,
        [1, 2, 3, 4, 5, 6],
        "every episode of every file exists as an item"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_files").await,
        3,
        "one file row per file"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_file_links").await,
        3,
        "E03, E05, E06 linked"
    );
    // E02 owns the double file, E03 links to the same file
    let f2: i64 = sqlx::query_scalar("SELECT id FROM media_files WHERE path LIKE '%E02E03%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let owner: i64 = sqlx::query_scalar("SELECT item_id FROM media_files WHERE id = ?")
        .bind(f2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, eps[1].0);
    let linked: Vec<i64> =
        sqlx::query_scalar("SELECT item_id FROM media_file_links WHERE file_id = ?")
            .bind(f2)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(linked, vec![eps[2].0]);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='season'"
        )
        .await,
        1
    );

    // rescans are idempotent; missing links of an old scan are repaired
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.unchanged, s.added), (3, 0));
    sqlx::query("DELETE FROM media_file_links")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM media_items WHERE kind='episode' AND episode_no IN (3, 5, 6)")
        .execute(&pool)
        .await
        .unwrap();
    scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='episode'"
        )
        .await,
        6,
        "episodes re-created"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_file_links").await,
        3
    );

    // a renamed multi-episode file keeps all of its episodes
    let before: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    std::fs::rename(
        &double,
        double.with_file_name("Some Show - S01E02E03 - 1080p.mkv"),
    )
    .unwrap();
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!((s.moved, s.added, s.removed), (1, 0, 0), "{s:?}");
    let after: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_file_links").await,
        3
    );

    // deleting the file removes both of its episodes
    std::fs::remove_file(double.with_file_name("Some Show - S01E02E03 - 1080p.mkv")).unwrap();
    let s = scanner.scan_library(&pool, lib).await.unwrap();
    assert_eq!(s.removed, 1);
    let left: Vec<i64> = sqlx::query_scalar(
        "SELECT episode_no FROM media_items WHERE kind='episode' ORDER BY episode_no",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(left, [1, 4, 5, 6]);
}

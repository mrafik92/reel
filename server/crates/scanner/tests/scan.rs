use std::{path::Path, process::Command, sync::Arc, time::Duration};

use async_trait::async_trait;
use reel_core::{ServerEvent, db};
use reel_scanner::{LibraryWatcher, MetadataProvider, Scanner};
use sqlx::SqlitePool;

fn have_tool(t: &str) -> bool {
    Command::new(t).arg("-version").output().is_ok()
}

fn make_video(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=1:size=64x48:rate=5")
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
        ])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed for {}", path.display());
}

async fn add_library(pool: &SqlitePool, kind: &str, root: &Path) -> i64 {
    let paths = serde_json::to_string(&[root.to_str().unwrap()]).unwrap();
    sqlx::query("INSERT INTO libraries (name, kind, paths_json) VALUES (?, ?, ?)")
        .bind(kind)
        .bind(kind)
        .bind(paths)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<i64>>);

#[async_trait]
impl MetadataProvider for Recorder {
    async fn enrich(&self, item_id: i64) -> reel_core::Result<()> {
        self.0.lock().unwrap().push(item_id);
        Ok(())
    }
}

#[tokio::test]
async fn scan_movies_and_shows() {
    if !have_tool("ffmpeg") || !have_tool("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let movies = tmp.path().join("movies");
    let shows = tmp.path().join("shows");
    let m1 = movies.join("Test Movie (2020)/Test Movie (2020).mkv");
    let m2 = movies.join("Test Movie (2020)/Test Movie (2020) - Director's Cut.mkv");
    make_video(&m1);
    make_video(&m2);
    std::fs::write(
        m1.with_extension("en.forced.srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nhi\n",
    )
    .unwrap();
    // ignored: sample
    make_video(&movies.join("Test Movie (2020)/Sample/sample.mkv"));
    let e1 = shows.join("Some Show (2019)/Season 01/Some Show - S01E01 - Pilot.mkv");
    let e2 = shows.join("Some Show (2019)/Season 01/Some Show - S01E02.mkv");
    make_video(&e1);
    make_video(&e2);

    let pool = db::connect_url("sqlite::memory:").await.unwrap();
    let ml = add_library(&pool, "movies", &movies).await;
    let sl = add_library(&pool, "shows", &shows).await;

    let (tx, mut rx) = tokio::sync::broadcast::channel(256);
    let rec = Arc::new(Recorder::default());
    let scanner = Scanner::new()
        .with_events(tx)
        .with_metadata(rec.clone())
        .with_concurrency(2);

    let s = scanner.scan_library(&pool, ml).await.unwrap();
    assert_eq!((s.added, s.failed, s.scanned), (2, 0, 2));
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_items WHERE kind='movie'").await,
        1
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM media_files").await, 2);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_files WHERE edition = 'Director''s Cut'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM streams WHERE kind='video' AND width=64 AND height=48"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM streams WHERE kind='audio' AND codec='aac'"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM streams WHERE kind='subtitle' AND forced=1 AND language='en' AND external_path IS NOT NULL"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_files WHERE probe_json IS NOT NULL AND container='mkv'"
        )
        .await,
        2
    );
    // events
    let mut last = None;
    while let Ok(ServerEvent::ScanProgress {
        library_id,
        scanned,
        total,
    }) = rx.try_recv()
    {
        assert_eq!(library_id, ml);
        last = Some((scanned, total));
    }
    assert_eq!(last, Some((2, 2)));

    let s = scanner.scan_library(&pool, sl).await.unwrap();
    assert_eq!((s.added, s.failed), (2, 0));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='show' AND title='Some Show' AND year=2019"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='season' AND season_no=1"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='episode' AND episode_no IN (1,2)"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='episode' AND title='Pilot'"
        )
        .await,
        1
    );
    // metadata hook: movie + show + season + 2 episodes
    assert_eq!(rec.0.lock().unwrap().len(), 5);

    // Incremental: nothing changes.
    let s = scanner.scan_library(&pool, sl).await.unwrap();
    assert_eq!((s.added, s.updated, s.unchanged, s.removed), (0, 0, 2, 0));
    assert_eq!(rec.0.lock().unwrap().len(), 5);

    // Delete an episode and rescan: file row gone, episode gone, season/show remain.
    std::fs::remove_file(&e2).unwrap();
    let s = scanner.scan_library(&pool, sl).await.unwrap();
    assert_eq!(s.removed, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_files WHERE path LIKE '%S01E02%'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_items WHERE kind='episode'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM streams WHERE file_id NOT IN (SELECT id FROM media_files)"
        )
        .await,
        0
    );

    // Delete the last episode via targeted rescan: whole hierarchy removed.
    std::fs::remove_file(&e1).unwrap();
    let s = scanner.scan_path(&pool, &e1).await.unwrap();
    assert_eq!(s.removed, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM media_items WHERE library_id = ?")
        .bind(sl)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);

    // Targeted add via scan_path.
    make_video(&e2);
    let s = scanner.scan_path(&pool, &e2).await.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM media_items WHERE kind='show'").await,
        1
    );

    // Missing library root must not delete anything.
    std::fs::remove_dir_all(&movies).unwrap();
    let s = scanner.scan_library(&pool, ml).await.unwrap();
    assert_eq!(s.removed, 0);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM media_files WHERE path LIKE '%movies%'"
        )
        .await,
        2
    );

    // Unknown path / library
    assert!(
        scanner
            .scan_path(&pool, Path::new("/nonexistent/x.mkv"))
            .await
            .is_err()
    );
    assert!(scanner.scan_library(&pool, 9999).await.is_err());
}

#[tokio::test]
async fn watcher_triggers_rescan() {
    if !have_tool("ffmpeg") || !have_tool("ffprobe") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("movies");
    std::fs::create_dir_all(&root).unwrap();
    let pool = db::connect_url("sqlite::memory:").await.unwrap();
    add_library(&pool, "movies", &root).await;
    let scanner = Scanner::new();
    let _w = LibraryWatcher::start(scanner, pool.clone(), Duration::from_millis(200))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    make_video(&root.join("Watched (2021)/Watched (2021).mkv"));
    let mut found = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if count(&pool, "SELECT count(*) FROM media_files").await == 1 {
            found = true;
            break;
        }
    }
    assert!(found, "watcher did not ingest the new file");
}

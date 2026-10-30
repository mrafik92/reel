#![allow(dead_code)]
use std::{path::Path, process::Command};

use sqlx::SqlitePool;

pub fn have_tool(t: &str) -> bool {
    Command::new(t).arg("-version").output().is_ok()
}

/// A tiny video of `secs` seconds. Different durations give different sizes/durations, so tests can
/// tell files apart.
pub fn make_video(path: &Path, secs: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=duration={secs}:size=64x48:rate=5"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=duration={secs}"))
        .args(["-c:v", "mpeg4", "-c:a", "aac"])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed for {}", path.display());
}

pub async fn add_library(pool: &SqlitePool, kind: &str, root: &Path) -> i64 {
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

pub async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

pub fn skip() -> bool {
    if !have_tool("ffmpeg") || !have_tool("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return true;
    }
    false
}

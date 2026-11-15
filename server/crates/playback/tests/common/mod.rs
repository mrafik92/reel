//! Shared helpers for the playback integration tests.
//!
//! Synthesizing test media with ffmpeg dominates the CPU time of these tests, and several tests
//! (and test binaries) need the very same file. nextest runs every test in its own process, so an
//! in-memory cache doesn't help: `ffmpeg_fixture` caches on disk instead, once per distinct
//! command line and ffmpeg build.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

pub fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Directory shared by all tests of this build for cached fixtures (and their inputs).
pub fn fixtures_dir() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("reel-fixtures");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-cue SRT file in [`fixtures_dir`], at a stable path so commands that use it hash the same.
pub fn srt() -> PathBuf {
    let p = fixtures_dir().join("one-cue.srt");
    if !p.exists() {
        let tmp = p.with_extension(format!(
            "{}.{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&tmp, "1\n00:00:01,000 --> 00:00:02,000\nhi\n").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
    }
    p
}

fn ffmpeg_version() -> String {
    let out = Command::new("ffmpeg").arg("-version").output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `ffmpeg <args> <dest>`, generated at most once per build: `cmd` is an `ffmpeg` command with
/// every argument except the output path (inputs must live at stable paths, see [`srt`]). The
/// result is keyed by the command line and the ffmpeg version, then copied to `dest` so each test
/// owns its file exactly as if it had run ffmpeg itself.
pub fn ffmpeg_fixture(mut cmd: Command, dest: &Path) -> PathBuf {
    let ext = dest
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bin".into());
    let mut h = DefaultHasher::new();
    format!("{cmd:?}").hash(&mut h);
    ffmpeg_version().hash(&mut h);
    ext.hash(&mut h);
    let dir = fixtures_dir();
    let key = format!("{:016x}", h.finish());
    let cached = dir.join(format!("{key}.{ext}"));
    let lock = dir.join(format!("{key}.{ext}.lock"));

    while !cached.exists() {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => {
                // Removes the lock even if ffmpeg fails, so waiting tests don't hang.
                struct Unlock<'a>(&'a Path);
                impl Drop for Unlock<'_> {
                    fn drop(&mut self) {
                        let _ = std::fs::remove_file(self.0);
                    }
                }
                let _unlock = Unlock(&lock);
                if cached.exists() {
                    break; // another process finished between our check and the lock
                }
                // Only the lock holder writes this name; same extension so ffmpeg picks the
                // same muxer.
                let tmp = dir.join(format!("{key}.tmp.{ext}"));
                let st = cmd.arg(&tmp).status().unwrap();
                assert!(st.success(), "ffmpeg fixture failed: {cmd:?}");
                std::fs::rename(&tmp, &cached).unwrap();
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // A lock left behind by a killed test run: take it over.
                let stale = std::fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > Duration::from_secs(600));
                if stale {
                    let _ = std::fs::remove_file(&lock);
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            Err(e) => panic!("fixture lock {}: {e}", lock.display()),
        }
    }
    // Marks the fixture as used: CI drops fixtures no test touched before saving its cache.
    if let Ok(f) = std::fs::File::options().write(true).open(&cached) {
        let _ = f.set_modified(SystemTime::now());
    }
    // A copy, not a hard link: a test that touches its file must not change the cached one.
    std::fs::copy(&cached, dest).unwrap();
    dest.to_path_buf()
}

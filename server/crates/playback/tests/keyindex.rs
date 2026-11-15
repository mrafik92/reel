//! Native keyframe index readers vs ffprobe, plus the fast-decide / background-index fallback.

use reel_playback::decision::*;
use reel_playback::keyindex::{self, Limited};
use reel_playback::model::*;
use reel_playback::plan::parse_keyframe_csv;
use reel_playback::profile::DeviceProfile;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

mod common;
use common::have_ffmpeg;

/// 60s, noisy 640x360 (so the file is tens of MB), irregular 37-frame GOP, B-frames.
fn make(dir: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let mut c = Command::new("ffmpeg");
    c.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args([
            "-i",
            "testsrc2=size=480x270:rate=25:duration=60,noise=alls=8:allf=t",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=60"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-bf",
            "2",
            "-g",
            "37",
            "-keyint_min",
            "37",
        ])
        .args([
            "-sc_threshold",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "96k",
        ])
        .args(extra);
    common::ffmpeg_fixture(c, &dir.join(name))
}

fn ffprobe_keyframes(path: &Path) -> Vec<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,dts_time,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    let k = parse_keyframe_csv(&String::from_utf8_lossy(&out.stdout));
    let first = k[0];
    k.into_iter().map(|x| x - first).collect()
}

fn check_native(path: &Path) {
    let expected = ffprobe_keyframes(path);
    let size = std::fs::metadata(path).unwrap().len();
    let mut r = Limited::new(
        std::fs::File::open(path).unwrap(),
        Some(Duration::from_secs(5)),
    );
    let counter = r.counter();
    let t = Instant::now();
    let got = keyindex::read_keyframes(&mut r)
        .unwrap()
        .expect("native index");
    let took = t.elapsed();
    let read = counter.load(Ordering::Relaxed);
    assert_eq!(
        got.len(),
        expected.len(),
        "{path:?}: {} vs {}",
        got.len(),
        expected.len()
    );
    for (a, b) in got.iter().zip(&expected) {
        assert!((a - b).abs() < 0.001, "{path:?}: {a} vs {b}");
    }
    assert!(read < 1_000_000, "{path:?}: read {read} bytes of {size}");
    assert!(read * 20 < size, "{path:?}: read {read} of {size}");
    assert!(took < Duration::from_millis(500), "took {took:?}");
    eprintln!(
        "{path:?}: {} keyframes, read {read} of {size} bytes in {took:?}",
        got.len()
    );
}

#[test]
fn native_index_matches_ffprobe_mkv_mp4_faststart_and_not() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    check_native(&make(tmp.path(), "a.mkv", &[]));
    check_native(&make(tmp.path(), "b.webm_like.mkv", &["-f", "matroska"]));
    check_native(&make(
        tmp.path(),
        "c_faststart.mp4",
        &["-movflags", "+faststart"],
    ));
    check_native(&make(tmp.path(), "d_moov_at_end.mp4", &[]));
}

#[test]
fn live_mkv_without_cues_has_no_index() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let f = make(tmp.path(), "live.mkv", &["-f", "matroska", "-live", "1"]);
    let mut r = std::fs::File::open(&f).unwrap();
    assert_eq!(keyindex::read_keyframes(&mut r).unwrap(), None);
}

/// Reader that sleeps on every read, like a very slow NFS mount.
struct Slow<R>(R, Duration);
impl<R: Read> Read for Slow<R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(self.1);
        self.0.read(b)
    }
}
impl<R: Seek> Seek for Slow<R> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(p)
    }
}

#[test]
fn slow_reader_hits_deadline_instead_of_hanging() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let f = make(tmp.path(), "slow.mkv", &[]);
    let slow = Slow(std::fs::File::open(&f).unwrap(), Duration::from_millis(150));
    let mut r = Limited::new(slow, Some(Duration::from_millis(400)));
    let t = Instant::now();
    let res = keyindex::read_keyframes(&mut r);
    assert!(res.is_err(), "expected timeout, got {res:?}");
    assert!(t.elapsed() < Duration::from_secs(2));
}

// ---------------------------------------------------------------------------------------------
// session level
// ---------------------------------------------------------------------------------------------

fn request(path: &Path, duration: f64) -> StartRequest {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(640);
    v.height = Some(360);
    v.bit_depth = Some(8);
    let mut a = StreamInfo::new(1, StreamKind::Audio, "aac");
    a.channels = Some(2);
    a.default = true;
    let file = FileInfo {
        id: 7,
        path: path.to_string_lossy().into(),
        container: Some("matroska,webm".into()),
        duration_s: Some(duration),
        bitrate: Some(2_000_000),
    };
    let streams = vec![v, a];
    let p = DeviceProfile::preset("web_chrome").unwrap();
    let decision = decide(&DecisionInput {
        file: &file,
        streams: &streams,
        audio_index: None,
        subtitle_index: None,
        profile: &p,
        max_bitrate_kbps: None,
    });
    assert_eq!(decision.mode, PlaybackMode::Remux);
    StartRequest {
        user_id: 1,
        device_id: None,
        file,
        streams,
        decision,
        start_position_s: 0.0,
        user_max_streams: None,
        user_max_transcodes: None,
        user_agent: None,
    }
}

#[tokio::test]
async fn cued_mkv_session_is_exact_and_fast() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let f = make(tmp.path(), "a.mkv", &[]);
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    let t = Instant::now();
    let info = mgr.create_session(request(&f, 60.0)).await.unwrap();
    let took = t.elapsed();
    eprintln!("create_session (copy mode, cued mkv): {took:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert!(!info.fallback_playlist);
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    assert!(
        tmp.path().join("keyframes").read_dir().unwrap().count() == 1,
        "index cached"
    );
    mgr.shutdown().await;
}

#[cfg(unix)]
fn slow_ffprobe(dir: &Path, secs: u32) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("slow-ffprobe.sh");
    std::fs::write(
        &p,
        format!("#!/bin/sh\nsleep {secs}\nexec ffprobe \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.to_string_lossy().into()
}

#[cfg(unix)]
#[tokio::test]
async fn no_index_falls_back_to_event_playlist_and_indexes_in_background() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let f = make(tmp.path(), "live.mkv", &["-f", "matroska", "-live", "1"]);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.ffprobe_path = slow_ffprobe(tmp.path(), 2); // simulate a slow full scan
    let mgr = SessionManager::new(cfg, HwAccel::Software);

    let t = Instant::now();
    let info = mgr.create_session(request(&f, 60.0)).await.unwrap();
    let took = t.elapsed();
    eprintln!("create_session (no index, fallback): {took:?}");
    assert!(
        took < Duration::from_secs(2),
        "decide must not wait for the scan: {took:?}"
    );
    assert!(info.fallback_playlist);
    assert_eq!(mgr.background_index_jobs(), 1);

    // a second session for the same file does not start another scan
    let info2 = mgr.create_session(request(&f, 60.0)).await.unwrap();
    assert!(info2.fallback_playlist);
    assert_eq!(mgr.background_index_jobs(), 1, "scan deduplicated per file");

    // playback starts immediately from ffmpeg's growing EVENT playlist
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:EVENT"), "{pl}");
    assert!(pl.contains("#EXT-X-START:TIME-OFFSET=0"));
    assert!(pl.contains("seg_00000.m4s"));
    let init = mgr.segment_path(&info.id, "init.mp4").await.unwrap();
    assert!(reel_playback::session::init_is_complete(
        &std::fs::read(init).unwrap()
    ));
    let seg = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(std::fs::metadata(seg).unwrap().len() > 0);

    // the background scan completes and the next session is exact
    let mut done = false;
    for _ in 0..100 {
        if tmp.path().join("keyframes").exists() && mgr.background_index_jobs() == 0 {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(done, "background index should finish");
    let info3 = mgr.create_session(request(&f, 60.0)).await.unwrap();
    assert!(!info3.fallback_playlist);
    mgr.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn background_scan_is_killed_when_sessions_end() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let f = make(tmp.path(), "live.mkv", &["-f", "matroska", "-live", "1"]);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.ffprobe_path = slow_ffprobe(tmp.path(), 30);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let a = mgr.create_session(request(&f, 60.0)).await.unwrap();
    let b = mgr.create_session(request(&f, 60.0)).await.unwrap();
    assert_eq!(mgr.background_index_jobs(), 1);
    mgr.remove(&a.id).await;
    assert_eq!(
        mgr.background_index_jobs(),
        1,
        "still needed by the other session"
    );
    mgr.remove(&b.id).await;
    assert_eq!(
        mgr.background_index_jobs(),
        0,
        "scan aborted with the last session"
    );
    let c = mgr.create_session(request(&f, 60.0)).await.unwrap();
    assert_eq!(mgr.background_index_jobs(), 1);
    mgr.shutdown().await;
    assert_eq!(mgr.background_index_jobs(), 0);
    let _ = c;
}

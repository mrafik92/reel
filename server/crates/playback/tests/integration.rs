//! Integration tests that need a real ffmpeg; they skip silently when it is missing.

use reel_playback::decision::*;
use reel_playback::hwaccel;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::subtitles::{SubtitleSource, to_vtt};
use reel_playback::{HwAccel, PlaybackError, SessionConfig, SessionManager, StartRequest};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod common;
use common::have_ffmpeg;

fn make_source(dir: &Path) -> PathBuf {
    let mut c = Command::new("ffmpeg");
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25:duration=12",
    ])
    .args([
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=12",
        "-c:v",
        "libx264",
        "-g",
        "25",
        "-pix_fmt",
        "yuv420p",
    ])
    .args(["-c:a", "ac3", "-ac", "2"]);
    common::ffmpeg_fixture(c, &dir.join("src.mkv"))
}

fn streams() -> Vec<StreamInfo> {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(320);
    v.height = Some(240);
    v.bit_depth = Some(8);
    let mut a = StreamInfo::new(1, StreamKind::Audio, "ac3");
    a.channels = Some(2);
    a.default = true;
    vec![v, a]
}

fn request(path: &Path, decision: PlaybackDecision, user: i64, start: f64) -> StartRequest {
    StartRequest {
        user_id: user,
        device_id: Some("dev".into()),
        file: FileInfo {
            id: 1,
            path: path.to_string_lossy().into(),
            container: Some("matroska,webm".into()),
            duration_s: Some(12.0),
            bitrate: Some(500_000),
        },
        streams: streams(),
        decision,
        start_position_s: start,
        user_max_streams: None,
        user_max_transcodes: None,
        user_agent: None,
    }
}

fn chrome_decision(path: &Path) -> PlaybackDecision {
    let p = DeviceProfile::preset("web_chrome").unwrap();
    let r = request(path, full_transcode(), 1, 0.0);
    decide(&DecisionInput {
        file: &r.file,
        streams: &r.streams,
        audio_index: None,
        subtitle_index: None,
        profile: &p,
        max_bitrate_kbps: None,
    })
}

fn full_transcode() -> PlaybackDecision {
    PlaybackDecision {
        mode: PlaybackMode::Transcode,
        video: VideoAction::Encode {
            codec: "h264".into(),
            width: 320,
            height: 240,
            bitrate_kbps: 500,
            tonemap: false,
        },
        audio: AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 128,
        },
        subtitle: SubtitleAction::None,
        reasons: vec![],
        video_index: Some(0),
        audio_index: Some(1),
        subtitle_index: None,
    }
}

async fn best_hw() -> HwAccel {
    let st = hwaccel::detect().await;
    // Prefer VideoToolbox when present (dev macs); otherwise software keeps the test deterministic.
    if st.available.contains(&HwAccel::VideoToolbox) {
        HwAccel::VideoToolbox
    } else {
        HwAccel::Software
    }
}

async fn wait_gone(p: &Path) -> bool {
    for _ in 0..50 {
        if !p.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn audio_transcode_session_serves_master_segments_and_seeks() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let decision = chrome_decision(&src);
    assert_eq!(decision.mode, PlaybackMode::AudioTranscode);

    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0; // force the restart path on a far-ahead request
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr
        .create_session(request(&src, decision, 1, 0.0))
        .await
        .unwrap();

    let master = mgr.master_playlist(&info.id).unwrap();
    assert!(master.contains("main/index.m3u8"));
    let media = mgr.media_playlist(&info.id).unwrap();
    assert_eq!(media.matches("#EXTINF").count(), 3);

    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let init = mgr.segment_path(&info.id, "init.mp4").await.unwrap();
    assert!(init.exists());
    let s0 = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(std::fs::metadata(s0).unwrap().len() > 0);

    // seek: segment 2 requested far ahead of the head
    let s2 = mgr.segment_path(&info.id, "seg_00002.m4s").await.unwrap();
    assert!(s2.exists());
    assert!(matches!(
        mgr.segment_path(&info.id, "seg_00009.m4s").await,
        Err(PlaybackError::Invalid(_))
    ));
    assert_eq!(mgr.list().await.len(), 1);

    mgr.update_progress(&info.id, 9.0, Some(700), false)
        .unwrap();
    assert_eq!(mgr.session(&info.id).await.unwrap().position_s, 9.0);

    assert!(mgr.remove(&info.id).await);
    assert!(wait_gone(&dir).await);
    assert!(matches!(
        mgr.media_playlist(&info.id),
        Err(PlaybackError::SessionNotFound)
    ));
}

#[tokio::test]
async fn full_transcode_with_start_offset_and_hw_choice() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let hw = best_hw().await;
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), hw);
    // start at 8s → ffmpeg starts at segment 2
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 8.0))
        .await
        .unwrap();
    let s2 = mgr.segment_path(&info.id, "seg_00002.m4s").await.unwrap();
    assert!(std::fs::metadata(&s2).unwrap().len() > 0);
    // timestamps stay absolute after the mid-file restart (segment 2 = 8s)
    let init = mgr.segment_path(&info.id, "init.mp4").await.unwrap();
    let pts = first_video_pts(&init, &s2, tmp.path());
    assert!((pts - 8.0).abs() < 0.2, "segment 2 starts at {pts}");
    // go back to segment 0: restarts ffmpeg at 0
    let s0 = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(s0.exists());
    mgr.shutdown().await;
}

#[tokio::test]
async fn reaper_cleans_idle_sessions() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.idle_timeout = Duration::from_millis(400);
    cfg.reaper_interval = Duration::from_millis(100);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let _reaper = mgr.spawn_reaper();
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(dir.exists());
    assert!(wait_gone(&dir).await, "idle session dir should be removed");
    assert!(mgr.list().await.is_empty());
}

/// #95: a paused player with a full buffer requests nothing. Its "paused" ping keeps the session
/// past the normal idle timeout; a session whose last ping said "playing" is still reaped, and so
/// is a paused one once the paused timeout runs out.
#[tokio::test]
async fn paused_sessions_get_the_longer_idle_timeout() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.idle_timeout = Duration::from_millis(300);
    cfg.paused_idle_timeout = Duration::from_millis(1500);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let paused = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    let playing = mgr
        .create_session(request(&src, full_transcode(), 2, 0.0))
        .await
        .unwrap();
    mgr.update_progress(&paused.id, 3.0, None, true).unwrap();
    mgr.update_progress(&playing.id, 3.0, None, false).unwrap();
    assert!(mgr.session(&paused.id).await.unwrap().client_paused);

    tokio::time::sleep(Duration::from_millis(600)).await;
    let reaped = mgr.reap_idle().await;
    assert_eq!(
        reaped,
        vec![playing.id.clone()],
        "only the playing-but-silent one"
    );
    let dir = mgr.session_dir(&paused.id).unwrap().unwrap();
    assert!(dir.exists(), "the paused session keeps its segments");

    // Resumed: a "playing" ping brings the short timeout back.
    mgr.update_progress(&paused.id, 3.0, None, false).unwrap();
    assert!(!mgr.session(&paused.id).await.unwrap().client_paused);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mgr.reap_idle().await, vec![paused.id.clone()]);
    assert!(!dir.exists());
}

/// A paused session whose client went away (tab closed while paused) is still freed once the
/// paused timeout runs out.
#[tokio::test]
async fn an_abandoned_paused_session_is_reaped_after_the_paused_timeout() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.idle_timeout = Duration::from_millis(200);
    cfg.paused_idle_timeout = Duration::from_millis(700);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    mgr.update_progress(&info.id, 1.0, None, true).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(mgr.reap_idle().await.is_empty());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mgr.reap_idle().await, vec![info.id.clone()]);
    assert!(mgr.list().await.is_empty());
}

#[tokio::test]
async fn concurrency_limits_enforced() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.max_user_transcodes = 1;
    cfg.max_global_transcodes = 2;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    mgr.create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    assert!(matches!(
        mgr.create_session(request(&src, full_transcode(), 1, 0.0))
            .await,
        Err(PlaybackError::UserLimit(1))
    ));
    mgr.create_session(request(&src, full_transcode(), 2, 0.0))
        .await
        .unwrap();
    assert!(matches!(
        mgr.create_session(request(&src, full_transcode(), 3, 0.0))
            .await,
        Err(PlaybackError::GlobalLimit(2))
    ));
    // direct-play sessions are tracked but never limited
    let mut d = full_transcode();
    d.mode = PlaybackMode::DirectPlay;
    mgr.create_session(request(&src, d, 1, 0.0)).await.unwrap();
    assert_eq!(mgr.list().await.len(), 3);
    mgr.shutdown().await;
}

/// #13: transcodes running outside the manager (live TV packagers) count against the same
/// per-user and global limits, and only while they run.
#[tokio::test]
async fn external_transcodes_count_against_the_limits() {
    if !have_ffmpeg() {
        return;
    }
    struct Live(std::sync::Mutex<(usize, usize)>);
    impl reel_playback::ExternalTranscodes for Live {
        fn counts(&self, user_id: i64) -> (usize, usize) {
            let (user, all) = *self.0.lock().unwrap();
            (if user_id == 1 { user } else { 0 }, all)
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.max_user_transcodes = 1;
    cfg.max_global_transcodes = 2;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    // user 1 watches a transcoded live channel
    let live = std::sync::Arc::new(Live(std::sync::Mutex::new((1, 1))));
    let ext: std::sync::Arc<dyn reel_playback::ExternalTranscodes> = live.clone();
    mgr.set_external_transcodes(std::sync::Arc::downgrade(&ext));
    assert!(matches!(
        mgr.create_session(request(&src, full_transcode(), 1, 0.0))
            .await,
        Err(PlaybackError::UserLimit(1))
    ));
    // two live packagers run server-wide (user 1's and another user's)
    *live.0.lock().unwrap() = (1, 2);
    assert!(matches!(
        mgr.create_session(request(&src, full_transcode(), 3, 0.0))
            .await,
        Err(PlaybackError::GlobalLimit(2))
    ));
    // a remux is not a transcode
    let mut remux = full_transcode();
    remux.mode = PlaybackMode::DirectPlay;
    mgr.create_session(request(&src, remux, 1, 0.0))
        .await
        .unwrap();
    // the live channels end: the slots are free again
    *live.0.lock().unwrap() = (0, 0);
    mgr.create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    assert_eq!(mgr.transcode_counts(1), (1, 1));
    assert_eq!(
        mgr.transcode_caps(Some(3)),
        (1, 2),
        "stricter of server and user cap"
    );
    assert_eq!(mgr.transcode_caps(None), (1, 2));
    // a dropped counter counts nothing
    drop(ext);
    drop(live);
    mgr.create_session(request(&src, full_transcode(), 2, 0.0))
        .await
        .unwrap();
    mgr.shutdown().await;
}

#[tokio::test]
async fn router_serves_playlists_and_segments() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    let app = reel_playback::router::stream_router(mgr.clone());
    for (uri, code) in [
        (format!("/stream/{}/master.m3u8", info.id), StatusCode::OK),
        (
            format!("/stream/{}/main/index.m3u8", info.id),
            StatusCode::OK,
        ),
        (format!("/stream/{}/main/init.mp4", info.id), StatusCode::OK),
        (
            format!("/stream/{}/main/seg_00000.m4s", info.id),
            StatusCode::OK,
        ),
        (
            "/stream/nope/master.m3u8".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/stream/{}/main/evil.txt", info.id),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let r = app
            .clone()
            .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), code, "{uri}");
    }
    mgr.shutdown().await;
}

#[tokio::test]
async fn hwaccel_detection_returns_consistent_status() {
    if !have_ffmpeg() {
        return;
    }
    let st = hwaccel::detect().await;
    assert!(st.selected == HwAccel::Software || st.available.contains(&st.selected));
    assert!(!st.ffmpeg_hwaccels.is_empty());
}

#[test]
fn hwaccel_output_parsers() {
    let hw = "Hardware acceleration methods:\nvideotoolbox\nvaapi\n";
    assert_eq!(hwaccel::parse_hwaccels(hw), vec!["videotoolbox", "vaapi"]);
    let enc = "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264\n V....D h264_vaapi           H.264 (VAAPI)\n A....D aac AAC\n";
    let set = hwaccel::parse_encoders(enc);
    assert!(set.contains("libx264") && set.contains("h264_vaapi") && set.contains("aac"));
    assert!(!set.contains("="));
    let args = hwaccel::test_encode_args(&HwAccel::vaapi("/dev/dri/renderD128")).join(" ");
    assert!(
        args.contains("-init_hw_device vaapi=va:/dev/dri/renderD128")
            && args.contains("-c:v h264_vaapi")
    );
}

#[tokio::test]
async fn srt_and_ass_convert_to_cached_vtt() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let srt = tmp.path().join("a.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,500\nHello <i>world</i>\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n").unwrap();
    let cache = tmp.path().join("cache");
    let src = SubtitleSource::External {
        path: srt.to_string_lossy().into(),
    };
    let out = to_vtt("ffmpeg", &src, &cache, "stream-7").await.unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.starts_with("WEBVTT"), "{text}");
    assert!(text.contains("00:01.000 --> 00:02.500"));
    assert!(text.contains("Hello"));
    // cached: second call returns same path without re-running (works even with a bogus ffmpeg)
    let again = to_vtt("/nonexistent/ffmpeg", &src, &cache, "stream-7")
        .await
        .unwrap();
    assert_eq!(again, out);

    // embedded text stream inside a container
    let mkv = tmp.path().join("s.mkv");
    let st = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:d=5:r=5",
            "-i",
        ])
        .arg(&srt)
        .args([
            "-c:v", "libx264", "-c:s", "srt", "-map", "0:v", "-map", "1:s",
        ])
        .arg(&mkv)
        .status()
        .unwrap();
    assert!(st.success());
    let emb = SubtitleSource::Embedded {
        path: mkv.to_string_lossy().into(),
        stream_index: 1,
    };
    let out = to_vtt("ffmpeg", &emb, &cache, "stream-8").await.unwrap();
    assert!(std::fs::read_to_string(out).unwrap().contains("Bye"));
}

// ---------------------------------------------------------------------------------------------
// copy-mode keyframe playlists, HW fallback, throttling
// ---------------------------------------------------------------------------------------------

/// 30s h264(+aac) with an irregular GOP: a keyframe every 37 frames at 25fps = 1.48s.
fn make_irregular_source(dir: &Path) -> PathBuf {
    let mut c = Command::new("ffmpeg");
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25:duration=30",
    ])
    .args([
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=330:duration=30",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
    ])
    .args([
        "-g",
        "37",
        "-keyint_min",
        "37",
        "-sc_threshold",
        "0",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "64k",
    ]);
    common::ffmpeg_fixture(c, &dir.join("irregular.mkv"))
}

fn extinfs(playlist: &str) -> Vec<f64> {
    playlist
        .lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .map(|l| l.trim_end_matches(',').parse::<f64>().unwrap())
        .collect()
}

/// First video pts (seconds) in init+segment.
fn first_video_pts(init: &Path, seg: &Path, tmp: &Path) -> f64 {
    let joined = tmp.join("probe.mp4");
    let mut bytes = std::fs::read(init).unwrap();
    bytes.extend(std::fs::read(seg).unwrap());
    std::fs::write(&joined, bytes).unwrap();
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ])
        .arg(&joined)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let v = text
        .lines()
        .filter_map(|l| l.trim().parse::<f64>().ok())
        .fold(f64::MAX, f64::min);
    let _ = std::fs::remove_file(&joined);
    v
}

#[tokio::test]
async fn copy_mode_playlist_follows_real_keyframes() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_irregular_source(tmp.path());
    let p = DeviceProfile::preset("web_chrome").unwrap();
    let mut req = request(&src, full_transcode(), 1, 0.0);
    req.file.duration_s = Some(30.0);
    req.streams[1] = {
        let mut a = StreamInfo::new(1, StreamKind::Audio, "aac");
        a.channels = Some(2);
        a.default = true;
        a
    };
    req.decision = decide(&DecisionInput {
        file: &req.file,
        streams: &req.streams,
        audio_index: None,
        subtitle_index: None,
        profile: &p,
        max_bitrate_kbps: None,
    });
    assert_eq!(req.decision.mode, PlaybackMode::Remux);

    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr.create_session(req).await.unwrap();

    let playlist = mgr.media_playlist(&info.id).unwrap();
    let durs = extinfs(&playlist);
    let sum: f64 = durs.iter().sum();
    assert!((sum - 30.0).abs() < 0.01, "EXTINF sum {sum}");
    // segments are whole GOPs (multiples of 1.48s), not the nominal 4s
    for d in &durs[..durs.len() - 1] {
        assert!(
            (d / 1.48 - (d / 1.48).round()).abs() < 0.02,
            "segment {d} is not a whole number of GOPs"
        );
        assert!(*d >= 3.9, "segment {d} too short");
    }
    assert!(playlist.contains("#EXT-X-TARGETDURATION:5"), "{playlist}");

    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let init = mgr.segment_path(&info.id, "init.mp4").await.unwrap();
    // request out of order: forces an ffmpeg restart at an exact keyframe for the far segment
    let order = [4usize, 0, 2, 1, 3];
    let mut starts = vec![0.0];
    for d in &durs {
        starts.push(starts.last().unwrap() + d);
    }
    for &n in order.iter().filter(|n| **n < durs.len()) {
        let seg = mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        let pts = first_video_pts(&init, &seg, tmp.path());
        assert!(
            (pts - starts[n]).abs() < 0.05,
            "segment {n} starts at {pts}, playlist says {}",
            starts[n]
        );
    }
    assert!(dir.join("init.mp4").exists());
    mgr.shutdown().await;
}

#[tokio::test]
async fn failing_hw_backend_falls_back_to_software_and_is_marked_degraded() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let bogus = HwAccel::vaapi("/dev/dri/renderD999");
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), bogus);
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    assert_eq!(info.hwaccel, "vaapi");
    let seg = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(std::fs::metadata(seg).unwrap().len() > 0);
    assert_eq!(mgr.session(&info.id).await.unwrap().hwaccel, "software");
    assert_eq!(mgr.degraded_backends(), vec!["vaapi".to_string()]);
    assert_eq!(mgr.effective_hwaccel(), HwAccel::Software);
    // later sessions start in software directly
    let again = mgr
        .create_session(request(&src, full_transcode(), 2, 0.0))
        .await
        .unwrap();
    assert_eq!(again.hwaccel, "software");
    // status overlay
    let mut detected = reel_playback::HwAccelStatus::software();
    detected.selected = HwAccel::vaapi("/dev/dri/renderD999");
    let st = mgr.hwaccel_status(&detected);
    assert_eq!(st.degraded, vec!["vaapi".to_string()]);
    assert_eq!(st.selected, HwAccel::Software);
    mgr.shutdown().await;
}

#[tokio::test]
async fn failing_low_power_encoder_steps_down_to_regular_vaapi_then_software() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    let lp = HwAccel::Vaapi {
        device: "/dev/dri/renderD999".into(),
        low_power: hwaccel::LowPower {
            h264: true,
            hevc: true,
        },
    };
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), lp.clone());
    let info = mgr
        .create_session(request(&src, full_transcode(), 1, 0.0))
        .await
        .unwrap();
    assert_eq!(info.hwaccel, "vaapi");
    assert_eq!(info.encoder.as_deref(), Some("h264_vaapi (low power)"));
    // the bogus device fails both VAAPI attempts; software produces the segment
    let seg = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(std::fs::metadata(seg).unwrap().len() > 0);
    let now = mgr.session(&info.id).await.unwrap();
    assert_eq!(now.hwaccel, "software");
    assert_eq!(now.encoder.as_deref(), Some("libx264"));
    assert_eq!(
        mgr.degraded_backends(),
        vec!["vaapi-lp".to_string(), "vaapi".to_string()]
    );
    assert_eq!(mgr.effective_hwaccel(), HwAccel::Software);
    let mut detected = reel_playback::HwAccelStatus::software();
    detected.selected = lp;
    assert_eq!(mgr.hwaccel_status(&detected).selected, HwAccel::Software);
    mgr.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn ffmpeg_is_paused_when_far_ahead_and_resumed_when_client_catches_up() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("long.mkv");
    let st = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x360:rate=25:duration=300",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=duration=300",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-c:a",
            "ac3",
        ])
        .arg(&src)
        .status()
        .unwrap();
    assert!(st.success());

    let mut cfg = SessionConfig::new(tmp.path());
    cfg.segment_secs = 1;
    cfg.max_ahead_segments = 3;
    cfg.throttle_interval = Duration::from_millis(20);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let mut req = request(&src, full_transcode(), 1, 0.0);
    req.file.duration_s = Some(300.0);
    if let VideoAction::Encode {
        width,
        height,
        bitrate_kbps,
        ..
    } = &mut req.decision.video
    {
        (*width, *height, *bitrate_kbps) = (1280, 720, 3000);
    }
    let info = mgr.create_session(req).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();

    let mut paused = false;
    for _ in 0..300 {
        if mgr.session(&info.id).await.unwrap().paused {
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(paused, "ffmpeg should have been paused");
    let produced = |d: &Path| {
        std::fs::read_dir(d)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("seg_")
            })
            .count()
    };
    let n1 = produced(&dir);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(produced(&dir), n1, "no progress while paused");
    assert!(n1 < 30, "should not have run far ahead, got {n1}");

    // client catches up: request the newest existing segment repeatedly
    let mut resumed = false;
    for _ in 0..40 {
        let newest = produced(&dir) as u32 - 1;
        mgr.segment_path(&info.id, &format!("seg_{newest:05}.m4s"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !mgr.session(&info.id).await.unwrap().paused {
            resumed = true;
            break;
        }
    }
    assert!(resumed, "ffmpeg should resume once the client catches up");
    mgr.shutdown().await;
}

#[tokio::test]
async fn init_mp4_is_never_served_incomplete() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_source(tmp.path());
    // both a transcode and a copy-mode (remux) session, requested immediately after creation
    for decision in [full_transcode(), chrome_decision(&src)] {
        let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
        let info = mgr
            .create_session(request(&src, decision, 1, 0.0))
            .await
            .unwrap();
        let app = reel_playback::router::stream_router(mgr.clone());
        let r = app
            .oneshot(
                Request::get(format!("/stream/{}/main/init.mp4", info.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), 10_000_000)
            .await
            .unwrap();
        assert!(body.len() > 16, "init.mp4 body is {} bytes", body.len());
        assert_eq!(&body[4..8], b"ftyp");
        assert!(reel_playback::session::init_is_complete(&body));
        mgr.shutdown().await;
    }
}

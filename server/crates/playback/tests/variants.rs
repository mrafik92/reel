//! Multi-variant sessions (#111): every encoded rung cuts exactly at the shared segment plan, so
//! segment `n` of every variant covers the same frames, also after independent restarts; rungs
//! start only when one of their files is requested.

use reel_playback::decision::*;
use reel_playback::fmp4;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest, VariantSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::have_ffmpeg;

/// 120 s, 23.976 fps H.264 with B-frames and irregular GOPs (keyframes where the encoder put
/// them, 20–61 frames apart) + E-AC3 5.1, in an MKV: a remux plan with uneven segments.
fn make_mkv(dir: &Path) -> PathBuf {
    let mut c = Command::new("ffmpeg");
    c.args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x272:rate=24000/1001:duration=120",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=120,aformat=channel_layouts=5.1",
        ])
        .args(["-map", "0:v", "-map", "1:a"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "30",
            "-bf",
            "3",
            "-g",
            "61",
            "-keyint_min",
            "20",
            "-sc_threshold",
            "0",
            "-force_key_frames",
            "expr:eq(mod(n,53),0)",
        ])
        .args(["-pix_fmt", "yuv420p", "-c:a", "eac3", "-b:a", "384k"]);
    common::ffmpeg_fixture(c, &dir.join("variants.mkv"))
}

fn streams() -> Vec<StreamInfo> {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(640);
    v.height = Some(272);
    v.bit_depth = Some(8);
    v.profile = Some("High".into());
    let mut a = StreamInfo::new(1, StreamKind::Audio, "eac3");
    a.channels = Some(6);
    a.default = true;
    vec![v, a]
}

fn request(path: &Path, primary_encodes: bool) -> StartRequest {
    let streams = streams();
    let file = FileInfo {
        id: 111,
        path: path.to_string_lossy().into(),
        container: Some("matroska,webm".into()),
        duration_s: Some(120.0),
        bitrate: Some(1_000_000),
    };
    let p = DeviceProfile::preset("web_chrome").unwrap();
    let mut decision = decide(&DecisionInput {
        file: &file,
        streams: &streams,
        audio_index: None,
        subtitle_index: None,
        profile: &p,
        max_bitrate_kbps: None,
    });
    assert_eq!(decision.video, VideoAction::Copy);
    if primary_encodes {
        decision.mode = PlaybackMode::Transcode;
        decision.video = encode(640, 272, 900);
    }
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

fn encode(width: u32, height: u32, bitrate_kbps: u32) -> VideoAction {
    VideoAction::Encode {
        codec: "h264".into(),
        width,
        height,
        bitrate_kbps,
        tonemap: false,
    }
}

/// A rung with the primary's audio and a smaller picture.
fn rung(req: &StartRequest, id: &str, w: u32, h: u32) -> VariantSpec {
    let mut d = req.decision.clone();
    d.mode = PlaybackMode::Transcode;
    d.video = encode(w, h, 400);
    VariantSpec {
        id: id.into(),
        decision: d,
        meta: None,
    }
}

fn manager(tmp: &Path) -> std::sync::Arc<SessionManager> {
    let mut cfg = SessionConfig::new(tmp);
    // every jump is a restart: exercises independent runs per variant
    cfg.lookahead_segments = 0;
    SessionManager::new(cfg, HwAccel::Software)
}

fn plan_starts(playlist: &str) -> Vec<f64> {
    let mut starts = vec![0.0];
    for d in playlist
        .lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .map(|l| l.trim_end_matches(',').parse::<f64>().unwrap())
    {
        starts.push(starts.last().unwrap() + d);
    }
    starts
}

/// Key-frame presentation times of `init + seg`, read independently with ffprobe.
fn keyframes(dir: &Path, init: &[u8], seg: &[u8]) -> Vec<f64> {
    let f = dir.join("probe.mp4");
    std::fs::write(&f, [init, seg].concat()).unwrap();
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(&f)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.ends_with("K__") || l.contains(",K"))
        .filter_map(|l| l.split(',').next()?.parse().ok())
        .collect()
}

struct Seg {
    first_pts: f64,
    duration: f64,
    keyframes: Vec<f64>,
}

async fn fetch(mgr: &SessionManager, sid: &str, variant: &str, n: usize, tmp: &Path) -> Seg {
    let seg = mgr
        .variant_segment_path(sid, variant, &format!("seg_{n:05}.m4s"))
        .await
        .unwrap_or_else(|e| panic!("{variant} segment {n}: {e}"));
    let init = std::fs::read(
        mgr.variant_segment_path(sid, variant, "init.mp4")
            .await
            .unwrap(),
    )
    .unwrap();
    let data = std::fs::read(seg).unwrap();
    let tracks = fmp4::init_tracks(&init);
    Seg {
        first_pts: fmp4::first_video_pts(&data, &tracks).unwrap(),
        duration: fmp4::video_duration(&data, &tracks).unwrap(),
        keyframes: keyframes(tmp, &init, &data),
    }
}

const FRAME: f64 = 1001.0 / 24000.0;

/// Every variant's segment `n` starts at the plan start and covers the plan length (to within a
/// frame); an encoded variant's segment is exactly one GOP (its only keyframe is the forced one),
/// while a copied one holds whole source GOPs.
fn check(variant: &str, n: usize, s: &Seg, starts: &[f64]) {
    let (a, b) = (starts[n], starts[n + 1]);
    let what = format!(
        "{variant} seg {n}: plan {a:.3}..{b:.3}, first pts {:.3}, duration {:.3}, keyframes {:?}",
        s.first_pts, s.duration, s.keyframes
    );
    assert!((s.first_pts - a).abs() < 0.002, "start: {what}");
    if n + 2 < starts.len() {
        assert!(
            (s.duration - (b - a)).abs() < FRAME + 0.002,
            "length: {what}"
        );
    }
    assert!(!s.keyframes.is_empty(), "no keyframe: {what}");
    if variant != "main" {
        assert_eq!(
            s.keyframes.len(),
            1,
            "one keyframe per rung segment: {what}"
        );
    }
}

#[tokio::test]
async fn rungs_on_a_remux_plan_cut_exactly_at_source_keyframes() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = manager(tmp.path());
    let req = request(&src, false);
    let rungs = vec![rung(&req, "r360", 426, 182), rung(&req, "r240", 284, 120)];
    let info = mgr.create_session_with_rungs(req, rungs).await.unwrap();
    assert_eq!(
        mgr.variant_ids(&info.id).unwrap(),
        vec!["main", "r360", "r240"]
    );
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    assert!(!info.fallback_playlist, "remux must have a keyframe plan");
    let starts = plan_starts(&pl);
    let total = starts.len() - 1;
    let lens: Vec<f64> = starts.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        lens.iter().any(|l| (l - lens[0]).abs() > 0.1),
        "the fixture must give uneven segments: {lens:?}"
    );
    // each variant jumps around on its own: independent restarts at different segments
    let far = total * 3 / 4;
    let orders: [(&str, Vec<usize>); 3] = [
        ("main", vec![0, 1, 2, far, far + 1, 5]),
        ("r360", vec![far, 2, 3, 0, 1, total - 1]),
        ("r240", vec![5, 6, 1, far + 1, 0]),
    ];
    let mut seen: std::collections::BTreeMap<usize, Vec<(String, f64)>> = Default::default();
    for (variant, order) in &orders {
        for &n in order {
            let s = fetch(&mgr, &info.id, variant, n, tmp.path()).await;
            check(variant, n, &s, &starts);
            seen.entry(n)
                .or_default()
                .push((variant.to_string(), s.duration));
        }
    }
    // the same segment on two variants covers the same span
    for (n, v) in &seen {
        if *n + 1 < total {
            for w in v.windows(2) {
                assert!(
                    (w[0].1 - w[1].1).abs() < FRAME + 0.002,
                    "segment {n} differs between variants: {v:?}"
                );
            }
        }
    }
    mgr.shutdown().await;
}

#[tokio::test]
async fn transcode_only_rungs_share_the_absolute_grid() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = manager(tmp.path());
    let req = request(&src, true);
    let rungs = vec![rung(&req, "r240", 284, 120)];
    let info = mgr.create_session_with_rungs(req, rungs).await.unwrap();
    let starts = plan_starts(&mgr.media_playlist_async(&info.id).await.unwrap());
    assert!(
        (starts[1] - 4.0).abs() < 1e-9,
        "fixed 4 s plan: {:?}",
        &starts[..3]
    );
    let total = starts.len() - 1;
    // restarts at segments whose start falls between two frames (13 × 4 s = 52 s is not on the
    // 23.976 fps frame grid), in a different order per variant
    for (variant, order) in [
        ("main", vec![13usize, 14, 0, 1, 20, 21, total - 1]),
        ("r240", vec![20usize, 21, 13, 14, 1, 0]),
    ] {
        for n in order {
            let s = fetch(&mgr, &info.id, variant, n, tmp.path()).await;
            // fixed grid: the forced keyframe is the first frame at/after 4n (within a frame),
            // and the session moved the run so its first segment starts exactly on the plan
            let what = format!(
                "{variant} seg {n}: first {:.3} dur {:.3} kfs {:?}",
                s.first_pts, s.duration, s.keyframes
            );
            assert!((s.first_pts - starts[n]).abs() < FRAME + 0.002, "{what}");
            assert_eq!(s.keyframes.len(), 1, "{what}");
            if n + 1 < total {
                assert!((s.duration - 4.0).abs() < FRAME + 0.002, "{what}");
            }
        }
    }
    mgr.shutdown().await;
}

#[tokio::test]
async fn rungs_start_only_when_requested_and_where_the_player_is() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = manager(tmp.path());
    let req = request(&src, false);
    let rungs = vec![rung(&req, "r240", 284, 120)];
    let info = mgr.create_session_with_rungs(req, rungs).await.unwrap();
    let rdir = mgr.variant_dir(&info.id, "r240").unwrap();
    assert!(mgr.variant_running(&info.id, "main").await.unwrap());
    assert!(!mgr.variant_running(&info.id, "r240").await.unwrap());
    // the primary plays on: the rung stays idle
    for n in [0usize, 1, 10, 11] {
        mgr.variant_segment_path(&info.id, "main", &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
    }
    assert!(!mgr.variant_running(&info.id, "r240").await.unwrap());
    assert!(!rdir.exists(), "nothing written for an idle rung");
    // a level switch asks for the init first: the rung starts at the player's position (11), not
    // at the beginning of the file
    mgr.variant_segment_path(&info.id, "r240", "init.mp4")
        .await
        .unwrap();
    assert!(mgr.variant_running(&info.id, "r240").await.unwrap());
    mgr.variant_segment_path(&info.id, "r240", "seg_00012.m4s")
        .await
        .unwrap();
    assert!(!rdir.join("seg_00000.m4s").exists(), "rung started at 0");
    // unknown variants are not served
    assert!(
        mgr.variant_segment_path(&info.id, "nope", "init.mp4")
            .await
            .is_err()
    );
    assert!(mgr.variant_playlist(&info.id, "nope").await.is_err());
    assert!(mgr.variant_playlist(&info.id, "r240").await.is_ok());
    mgr.shutdown().await;
    assert!(!rdir.exists(), "session removal deletes rung output");
}

#[tokio::test]
async fn bad_rungs_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = manager(tmp.path());
    let req = request(Path::new("/nonexistent.mkv"), false);
    for spec in [
        rung(&req, "main", 284, 120),
        rung(&req, "../x", 284, 120),
        rung(&req, "", 284, 120),
        VariantSpec {
            id: "copy".into(),
            decision: req.decision.clone(),
            meta: None,
        },
    ] {
        assert!(
            mgr.create_session_with_rungs(req.clone(), vec![spec.clone()])
                .await
                .is_err(),
            "{:?} accepted",
            spec.id
        );
    }
    let dup = vec![rung(&req, "a", 284, 120), rung(&req, "a", 284, 120)];
    assert!(mgr.create_session_with_rungs(req, dup).await.is_err());
}

// ---- multi-variant sessions: lazy admission, idle stop, accounting (#111 part 2) ----------

use reel_playback::PlaybackError;
use reel_playback::session::AdaptiveRequest;
use reel_playback::variants::VariantMeta;

fn meta(label: &str, w: u32, h: u32, kbps: u32, codecs: &str) -> VariantMeta {
    VariantMeta {
        label: label.into(),
        width: w,
        height: h,
        average_kbps: kbps,
        peak_kbps: kbps + kbps / 5,
        codecs: Some(codecs.into()),
    }
}

/// Remux original (video and audio copied: holds no transcode slot) + one H.264 rung.
fn adaptive(src: &Path, user_id: i64, start: &str) -> (StartRequest, AdaptiveRequest) {
    let mut req = request(src, false);
    req.user_id = user_id;
    req.decision.mode = PlaybackMode::Remux;
    req.decision.audio = AudioAction::Copy;
    let low = rung(&req, "low", 284, 120);
    let a = AdaptiveRequest {
        primary_id: "original".into(),
        primary_meta: meta("Original", 640, 272, 1000, "avc1.640028,ec-3"),
        rungs: vec![VariantSpec {
            meta: Some(meta("240p", 284, 120, 400, "avc1.64001F,ec-3")),
            ..low
        }],
        start: start.into(),
    };
    (req, a)
}

fn capped_manager(
    tmp: &Path,
    global: usize,
    idle: std::time::Duration,
) -> std::sync::Arc<SessionManager> {
    let mut cfg = SessionConfig::new(tmp);
    cfg.max_global_transcodes = global;
    cfg.variant_idle_timeout = idle;
    SessionManager::new(cfg, HwAccel::Software)
}

#[tokio::test]
async fn rungs_hold_a_transcode_slot_only_while_they_run() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = capped_manager(tmp.path(), 1, std::time::Duration::from_millis(1500));
    let (r1, a1) = adaptive(&src, 1, "original");
    let s1 = mgr.create_session_adaptive(r1, a1).await.unwrap();
    let (r2, a2) = adaptive(&src, 2, "original");
    let s2 = mgr.create_session_adaptive(r2, a2).await.unwrap();
    // two remux sessions: no transcode slot used, the original runs, the rung is idle
    assert_eq!(mgr.transcode_counts(1), (0, 0));
    assert!(mgr.variant_running(&s1.id, "original").await.unwrap());
    assert!(!mgr.variant_running(&s1.id, "low").await.unwrap());
    let info = mgr.session(&s1.id).await.unwrap();
    let states: Vec<(String, String)> = info
        .variants
        .iter()
        .map(|v| (v.id.clone(), v.state.clone()))
        .collect();
    assert_eq!(
        states,
        vec![
            ("original".into(), "running".into()),
            ("low".into(), "idle".into())
        ]
    );
    assert_eq!(info.variants[1].bandwidth_kbps, 400);

    // the master lists both, ascending, with codecs
    let m = mgr.master_playlist(&s1.id).unwrap();
    let low_at = m.find("low/index.m3u8").unwrap();
    let orig_at = m.find("original/index.m3u8").unwrap();
    assert!(low_at < orig_at, "{m}");
    assert!(m.contains("BANDWIDTH=480000,AVERAGE-BANDWIDTH=400000,RESOLUTION=284x120,CODECS=\"avc1.64001F,ec-3\"\nlow/index.m3u8"), "{m}");

    // session 1 switches to the rung: it is admitted and takes the only slot
    mgr.variant_segment_path(&s1.id, "low", "seg_00001.m4s")
        .await
        .unwrap();
    assert_eq!(mgr.transcode_counts(1), (1, 1));
    let info = mgr.session(&s1.id).await.unwrap();
    assert_eq!(info.delivering.as_deref(), Some("low"));
    assert!(
        info.encoder.is_some(),
        "encoder follows the delivered variant"
    );
    assert_eq!(mgr.transcode_room(2, None), 0);

    // session 2's rung is refused (429), nothing is spawned for it
    let e = mgr
        .variant_segment_path(&s2.id, "low", "seg_00001.m4s")
        .await
        .unwrap_err();
    assert!(matches!(e, PlaybackError::VariantRefused(_)), "{e:?}");
    assert!(!mgr.variant_running(&s2.id, "low").await.unwrap());
    // its original keeps working
    mgr.variant_segment_path(&s2.id, "original", "seg_00001.m4s")
        .await
        .unwrap();

    // session 1 goes back to the original; after the idle timeout its rung is stopped and the
    // slot is free again (the original stays: it is still being fetched)
    for _ in 0..4 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        mgr.variant_segment_path(&s1.id, "original", "seg_00002.m4s")
            .await
            .unwrap();
    }
    let stopped = mgr.stop_idle_variants().await;
    assert!(
        stopped.contains(&(s1.id.clone(), "low".to_string())),
        "{stopped:?}"
    );
    assert!(!stopped.contains(&(s1.id.clone(), "original".to_string())));
    assert!(!mgr.variant_running(&s1.id, "low").await.unwrap());
    assert_eq!(mgr.transcode_counts(1), (0, 0));
    // now session 2 may switch down
    mgr.variant_segment_path(&s2.id, "low", "seg_00003.m4s")
        .await
        .unwrap();
    assert_eq!(mgr.transcode_counts(2), (1, 1));
    // session 1's rung: what it already produced is still served (no ffmpeg needed) ...
    let ldir = mgr.variant_dir(&s1.id, "low").unwrap();
    assert!(ldir.join("seg_00001.m4s").exists());
    mgr.variant_segment_path(&s1.id, "low", "seg_00001.m4s")
        .await
        .unwrap();
    assert!(!mgr.variant_running(&s1.id, "low").await.unwrap());
    // ... but anything that needs its ffmpeg again is refused until session 2's rung stops
    let total = plan_starts(&mgr.media_playlist_async(&s1.id).await.unwrap()).len() - 1;
    let missing = (0..total)
        .rev()
        .find(|n| !ldir.join(format!("seg_{n:05}.m4s")).exists())
        .expect("a segment the rung never produced");
    assert!(matches!(
        mgr.variant_segment_path(&s1.id, "low", &format!("seg_{missing:05}.m4s"))
            .await,
        Err(PlaybackError::VariantRefused(_))
    ));
    mgr.shutdown().await;
}

#[tokio::test]
async fn adaptive_session_prestarts_its_start_variant_and_counts_it() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = capped_manager(tmp.path(), 1, std::time::Duration::from_secs(30));
    // start low: the rung runs right away, the original is idle
    let (r1, a1) = adaptive(&src, 1, "low");
    let s1 = mgr.create_session_adaptive(r1, a1).await.unwrap();
    assert!(mgr.variant_running(&s1.id, "low").await.unwrap());
    assert!(!mgr.variant_running(&s1.id, "original").await.unwrap());
    assert_eq!(mgr.transcode_counts(1), (1, 1));
    // a second session that would start on a rung is refused at creation (cap 1)
    let (r2, a2) = adaptive(&src, 2, "low");
    assert!(matches!(
        mgr.create_session_adaptive(r2, a2).await,
        Err(PlaybackError::GlobalLimit(1))
    ));
    // one that starts on the remux original is fine
    let (r3, a3) = adaptive(&src, 2, "original");
    mgr.create_session_adaptive(r3, a3).await.unwrap();
    // the original starts lazily, without a transcode slot
    mgr.variant_segment_path(&s1.id, "original", "seg_00000.m4s")
        .await
        .unwrap();
    assert_eq!(mgr.transcode_counts(1), (1, 1));
    // removing the session frees the slot
    mgr.remove(&s1.id).await;
    assert_eq!(mgr.transcode_counts(1), (0, 0));
    mgr.shutdown().await;
}

#[tokio::test]
async fn adaptive_request_without_a_plan_is_single_variant_under_its_own_id() {
    // a remote-like input with no readable index can't be multi-variant: only the original stays,
    // and the master names it
    let tmp = tempfile::tempdir().unwrap();
    if !have_ffmpeg() {
        return;
    }
    let src = make_mkv(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.keyframe_fast_timeout = std::time::Duration::from_millis(0);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let (r, a) = adaptive(&src, 1, "low");
    let s = mgr.create_session_adaptive(r, a).await.unwrap();
    if s.fallback_playlist {
        assert_eq!(mgr.variant_ids(&s.id).unwrap(), vec!["original"]);
        let m = mgr.master_playlist(&s.id).unwrap();
        assert!(m.contains("\noriginal/index.m3u8\n"), "{m}");
        assert!(s.variants.is_empty());
    }
    mgr.shutdown().await;
}

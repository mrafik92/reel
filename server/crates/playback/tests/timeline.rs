//! Fragment timeline checks: after (re)starts, tfdt of both tracks must match the playlist.

use reel_playback::decision::*;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::have_ffmpeg;

/// 150s H.264 (irregular GOP) + E-AC3 5.1 + `nsubs` SRT tracks in an MKV.
fn make_mkv(dir: &Path, nsubs: usize) -> PathBuf {
    let srt = common::srt();
    let mut c = Command::new("ffmpeg");
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x272:rate=24000/1001:duration=150",
    ])
    .args([
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:duration=150,aformat=channel_layouts=5.1",
    ]);
    for _ in 0..nsubs {
        c.arg("-i").arg(&srt);
    }
    c.args(["-map", "0:v", "-map", "1:a"]);
    for i in 0..nsubs {
        c.args(["-map", &format!("{}:s", i + 2)]);
    }
    c.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "28",
        "-bf",
        "2",
        "-g",
        "57",
        "-keyint_min",
        "57",
        "-sc_threshold",
        "0",
    ])
    .args([
        "-pix_fmt", "yuv420p", "-c:a", "eac3", "-b:a", "384k", "-c:s", "srt",
    ]);
    common::ffmpeg_fixture(c, &dir.join("movie.mkv"))
}

// ---- minimal ISO-BMFF parsing -------------------------------------------------------------

fn boxes(d: &[u8]) -> Vec<(usize, usize, [u8; 4])> {
    let mut v = Vec::new();
    let mut p = 0;
    while p + 8 <= d.len() {
        let s = u32::from_be_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        if s < 8 || p + s > d.len() {
            break;
        }
        v.push((p + 8, p + s, d[p + 4..p + 8].try_into().unwrap()));
        p += s;
    }
    v
}

fn find<'a>(d: &'a [u8], k: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(d)
        .into_iter()
        .find(|b| &b.2 == k)
        .map(|b| &d[b.0..b.1])
}

fn be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(d[at..at + 4].try_into().unwrap())
}

/// track_id -> (timescale, is_video)
fn init_tracks(init: &[u8]) -> HashMap<u32, (u32, bool)> {
    let moov = find(init, b"moov").expect("moov");
    let mut out = HashMap::new();
    for (s, e, k) in boxes(moov) {
        if &k != b"trak" {
            continue;
        }
        let trak = &moov[s..e];
        let tkhd = find(trak, b"tkhd").unwrap();
        let id = if tkhd[0] == 1 {
            be32(tkhd, 20)
        } else {
            be32(tkhd, 12)
        };
        let mdia = find(trak, b"mdia").unwrap();
        let mdhd = find(mdia, b"mdhd").unwrap();
        let ts = if mdhd[0] == 1 {
            be32(mdhd, 20)
        } else {
            be32(mdhd, 12)
        };
        let hdlr = find(mdia, b"hdlr").unwrap();
        out.insert(id, (ts, &hdlr[8..12] == b"vide"));
    }
    out
}

/// Per track: (is_video, tfdt seconds, min presentation time, end time) over all fragments.
fn fragment_times(init: &[u8], seg: &[u8]) -> Vec<(u32, bool, f64, f64, f64)> {
    let tracks = init_tracks(init);
    let mut agg: HashMap<u32, (bool, f64, f64, f64)> = HashMap::new();
    for (s, e, k) in boxes(seg) {
        if &k != b"moof" {
            continue;
        }
        let moof = &seg[s..e];
        for (ts, te, tk) in boxes(moof) {
            if &tk != b"traf" {
                continue;
            }
            let traf = &moof[ts..te];
            let tfhd = find(traf, b"tfhd").unwrap();
            let id = be32(tfhd, 4);
            let tfhd_flags = be32(tfhd, 0) & 0xFFFFFF;
            let mut off = 8;
            if tfhd_flags & 1 != 0 {
                off += 8;
            }
            if tfhd_flags & 2 != 0 {
                off += 4;
            }
            let def_dur = if tfhd_flags & 8 != 0 {
                be32(tfhd, off)
            } else {
                0
            };
            let tfdt = find(traf, b"tfdt").expect("tfdt");
            let base = if tfdt[0] == 1 {
                u64::from_be_bytes(tfdt[4..12].try_into().unwrap())
            } else {
                be32(tfdt, 4) as u64
            };
            let (scale, video) = tracks[&id];
            let trun = find(traf, b"trun").expect("trun");
            let (flags, ver) = (be32(trun, 0) & 0xFFFFFF, trun[0]);
            let n = be32(trun, 4) as usize;
            let mut p = 8;
            if flags & 1 != 0 {
                p += 4;
            }
            if flags & 4 != 0 {
                p += 4;
            }
            let mut dts = base as i64;
            let mut first_pts = i64::MAX;
            for _ in 0..n {
                let dur = if flags & 0x100 != 0 {
                    let v = be32(trun, p);
                    p += 4;
                    v
                } else {
                    def_dur
                };
                if flags & 0x200 != 0 {
                    p += 4;
                }
                if flags & 0x400 != 0 {
                    p += 4;
                }
                let cts = if flags & 0x800 != 0 {
                    let v = be32(trun, p);
                    p += 4;
                    if ver == 1 { v as i32 as i64 } else { v as i64 }
                } else {
                    0
                };
                if first_pts == i64::MAX {
                    first_pts = dts + cts; // first sample in decode order
                }
                dts += dur as i64;
            }
            let (sc, tdt) = (scale as f64, base as f64 / scale as f64);
            let ent = agg.entry(id).or_insert((video, tdt, f64::MAX, 0.0));
            if tdt <= ent.1 {
                ent.2 = first_pts as f64 / sc;
            }
            ent.3 = ent.3.max(dts as f64 / sc);
        }
    }
    agg.into_iter()
        .map(|(id, (v, t, m, e))| (id, v, t, m, e))
        .collect()
}

/// (tfdt, first presentation time, end) of the video/audio track.
fn track(times: &[(u32, bool, f64, f64, f64)], video: bool) -> (f64, f64, f64) {
    let t = times.iter().find(|t| t.1 == video).expect("track");
    (t.2, t.3, t.4)
}

fn extinfs(p: &str) -> Vec<f64> {
    p.lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .map(|l| l.trim_end_matches(',').parse().unwrap())
        .collect()
}

fn request(path: &Path, nsubs: usize, start: f64) -> StartRequest {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(640);
    v.height = Some(272);
    v.bit_depth = Some(8);
    v.profile = Some("High".into());
    let mut a = StreamInfo::new(1, StreamKind::Audio, "eac3");
    a.channels = Some(6);
    a.default = true;
    let mut streams = vec![v, a];
    for i in 0..nsubs {
        streams.push(StreamInfo::new(
            2 + i as i64,
            StreamKind::Subtitle,
            "subrip",
        ));
    }
    let file = FileInfo {
        id: 291,
        path: path.to_string_lossy().into(),
        container: Some("matroska,webm".into()),
        duration_s: Some(150.0),
        bitrate: Some(1_000_000),
    };
    let p = DeviceProfile::preset("web_chrome").unwrap();
    let decision = decide(&DecisionInput {
        file: &file,
        streams: &streams,
        audio_index: None,
        subtitle_index: None,
        profile: &p,
        max_bitrate_kbps: None,
    });
    assert_eq!(decision.mode, PlaybackMode::AudioTranscode);
    assert_eq!(decision.video, VideoAction::Copy);
    StartRequest {
        user_id: 1,
        device_id: None,
        file,
        streams,
        decision,
        start_position_s: start,
        user_max_streams: None,
        user_max_transcodes: None,
        user_agent: None,
    }
}

#[tokio::test]
async fn timeline_after_restarts_matches_playlist() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path(), 45);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr.create_session(request(&src, 45, 0.0)).await.unwrap();
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    let durs = extinfs(&pl);
    let mut starts = vec![0.0];
    for d in &durs {
        starts.push(starts.last().unwrap() + d);
    }
    let n_last = durs.len() - 1;
    let far = (n_last * 3 / 4).max(5);
    let lax = std::env::var("TL_LAX").is_ok();
    let mut inits: Vec<Vec<u8>> = Vec::new();
    let mut report = Vec::new();
    for n in [0usize, 1, far, 2, far, n_last] {
        let seg = mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        let init = std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap();
        inits.push(init.clone());
        let t = fragment_times(&init, &std::fs::read(seg).unwrap());
        let ((vt, vp, ve), (at, ap, ae)) = (track(&t, true), track(&t, false));
        let end = starts[n + 1];
        report.push(format!(
            "seg {n}: plan {:.3}..{:.3} | video tfdt {vt:.3} pts {vp:.3} end {ve:.3} | audio tfdt {at:.3} pts {ap:.3} end {ae:.3}",
            starts[n], end
        ));
        if !lax {
            let r = report.last().unwrap();
            assert!((vp - starts[n]).abs() < 0.002, "video start: {r}");
            // audio may lead (muxer interleave) but must never start after the video (would leave a
            // hole at the seek position: buffered range empty, readyState 1)
            assert!(
                ap - starts[n] < 0.05 && ap - starts[n] > -0.15,
                "audio start: {r}"
            );
            assert!((vt - starts[n]).abs() < 0.2, "video tfdt: {r}");
            assert!(
                (ve - end).abs() < 0.15 && (ae - end).abs() < 0.15,
                "ends: {r}"
            );
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(
        inits.windows(2).all(|w| w[0] == w[1]),
        "init.mp4 changed across restarts"
    );
    mgr.shutdown().await;
}

/// ffmpeg's own (last-run) init.mp4 must equal the one served from the first run: same tracks,
/// timescales and no edit lists; only video+audio are mapped (the 45 subtitle tracks are not).
#[tokio::test]
async fn init_is_stable_across_restarts_and_maps_only_selected_streams() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path(), 45);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr.create_session(request(&src, 45, 0.0)).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let served = std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap();
    assert_eq!(init_tracks(&served).len(), 2, "only video + audio tracks");
    assert!(
        !served.windows(4).any(|w| w == b"elst"),
        "no edit list in init"
    );
    for n in [20usize, 3, 25] {
        mgr.segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        // ffmpeg's raw init of this run (wait until fully written)
        let mut raw = Vec::new();
        for _ in 0..50 {
            raw = std::fs::read(dir.join("init.mp4")).unwrap_or_default();
            if reel_playback::session::init_is_complete(&raw) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(raw, served, "restart at {n} produced a different init.mp4");
    }
    mgr.shutdown().await;
}

/// Resuming at `start_position_s` starts ffmpeg at the right *plan* segment directly.
#[tokio::test]
async fn start_position_starts_at_plan_segment_without_early_segments() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path(), 3);
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    let info = mgr.create_session(request(&src, 3, 86.9)).await.unwrap();
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    let durs = extinfs(&pl);
    let mut starts = vec![0.0];
    for d in &durs {
        starts.push(starts.last().unwrap() + d);
    }
    let n = starts.iter().rposition(|s| *s <= 86.9 + 0.001).unwrap();
    assert!(starts[n] <= 86.9 && starts[n + 1] > 86.9);
    let seg = mgr
        .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
        .await
        .unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    assert!(seg.exists());
    // no restart happened: nothing from the beginning of the file was produced
    assert!(!dir.join("seg_00000.m4s").exists() && !dir.join("gop_00000.m4s").exists());
    let init = std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap();
    let t = fragment_times(&init, &std::fs::read(seg).unwrap());
    let (vt, vp, _) = track(&t, true);
    assert!(
        (vp - starts[n]).abs() < 0.002,
        "video {vp} vs plan {} (tfdt {vt})",
        starts[n]
    );
    mgr.shutdown().await;
}

/// Same timeline guarantees for the encode path (fixed 4s grid).
#[tokio::test]
async fn encode_path_timeline_matches_fixed_grid_after_restarts() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path(), 2);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let mut req = request(&src, 2, 0.0);
    req.decision.mode = PlaybackMode::Transcode;
    req.decision.video = VideoAction::Encode {
        codec: "h264".into(),
        width: 640,
        height: 272,
        bitrate_kbps: 1500,
        tonemap: false,
    };
    let info = mgr.create_session(req).await.unwrap();
    let mut report = Vec::new();
    for n in [0usize, 1, 20, 3, 20] {
        let seg = mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        let init = std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap();
        let t = fragment_times(&init, &std::fs::read(seg).unwrap());
        let ((_, vp, ve), (_, ap, ae)) = (track(&t, true), track(&t, false));
        let plan = n as f64 * 4.0;
        report.push(format!(
            "seg {n}: plan {plan:.3} video {vp:.3}..{ve:.3} audio {ap:.3}..{ae:.3}"
        ));
        assert!((vp - plan).abs() < 0.1, "{}", report.last().unwrap());
        assert!(
            ap - plan < 0.05 && ap - plan > -0.2,
            "{}",
            report.last().unwrap()
        );
    }
    eprintln!("{}", report.join("\n"));
    mgr.shutdown().await;
}

//! Every segment we serve must be a clean fMP4 media segment for the (single, stable) init:
//! `[styp] (moof mdat)+`, no ftyp/moov/sidx/ts, known track ids, strictly increasing fragment
//! sequence numbers, decodable (init + segments in playlist order and across seek-restarts), and
//! no leftovers of older ffmpeg runs after a restart.

use reel_playback::decision::*;
use reel_playback::fmp4::{clean_segment, init_tracks};
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn make_mkv(dir: &Path) -> PathBuf {
    let srt = dir.join("s.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nhi\n").unwrap();
    let out = dir.join("movie.mkv");
    let mut c = Command::new("ffmpeg");
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x536:rate=24000/1001:duration=180",
    ])
    .args([
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:duration=180,aformat=channel_layouts=5.1",
    ]);
    for _ in 0..45 {
        c.arg("-i").arg(&srt);
    }
    c.args(["-map", "0:v", "-map", "1:a"]);
    for i in 0..45 {
        c.args(["-map", &format!("{}:s", i + 2)]);
    }
    // irregular GOPs: scene-cut off, 37 frames
    c.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "30",
        "-bf",
        "2",
        "-g",
        "37",
        "-keyint_min",
        "20",
        "-sc_threshold",
        "0",
    ])
    .args([
        "-pix_fmt", "yuv420p", "-c:a", "eac3", "-b:a", "384k", "-c:s", "srt",
    ])
    .arg(&out);
    assert!(c.status().unwrap().success());
    out
}

fn request(path: &Path, encode: bool) -> StartRequest {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(1280);
    v.height = Some(536);
    v.bit_depth = Some(8);
    v.profile = Some("High".into());
    let mut a = StreamInfo::new(1, StreamKind::Audio, "eac3");
    a.channels = Some(6);
    a.default = true;
    let mut streams = vec![v, a];
    for i in 0..45 {
        streams.push(StreamInfo::new(2 + i, StreamKind::Subtitle, "subrip"));
    }
    let file = FileInfo {
        id: 291,
        path: path.to_string_lossy().into(),
        container: Some("matroska,webm".into()),
        duration_s: Some(180.0),
        bitrate: Some(2_000_000),
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
    if encode {
        decision.mode = PlaybackMode::Transcode;
        decision.video = VideoAction::Encode {
            codec: "h264".into(),
            width: 1280,
            height: 536,
            bitrate_kbps: 2500,
            tonemap: false,
        };
    }
    StartRequest {
        user_id: 1,
        device_id: None,
        file,
        streams,
        decision,
        start_position_s: 86.9,
    }
}

fn extinfs(p: &str) -> Vec<f64> {
    p.lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .map(|l| l.trim_end_matches(',').parse().unwrap())
        .collect()
}

/// Video pts range (first, last+frame) and any decode errors of `init + segs`.
fn probe_video(tmp: &Path, init: &[u8], segs: &[Vec<u8>]) -> (f64, f64) {
    let f = tmp.join("chain.mp4");
    let mut bytes = init.to_vec();
    for s in segs {
        bytes.extend_from_slice(s);
    }
    std::fs::write(&f, bytes).unwrap();
    // full decode, any error = unparseable / corrupt
    let dec = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&f)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        dec.status.success() && dec.stderr.is_empty(),
        "decode errors: {}",
        String::from_utf8_lossy(&dec.stderr)
    );
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
        .arg(&f)
        .output()
        .unwrap();
    let t: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    (
        t.iter().cloned().fold(f64::MAX, f64::min),
        t.iter().cloned().fold(f64::MIN, f64::max),
    )
}

fn mfhd_sequences(seg: &[u8]) -> Vec<u32> {
    let mut v = Vec::new();
    let mut p = 0;
    while p + 8 <= seg.len() {
        let s = u32::from_be_bytes(seg[p..p + 4].try_into().unwrap()) as usize;
        if &seg[p + 4..p + 8] == b"moof" {
            // moof > mfhd: first child
            assert_eq!(&seg[p + 12..p + 16], b"mfhd");
            v.push(u32::from_be_bytes(seg[p + 20..p + 24].try_into().unwrap()));
        }
        p += s;
    }
    v
}

async fn run(encode: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    // the user flow: resume at 86.9s, then segments from the start, then a far seek, then back
    let info = mgr.create_session(request(&src, encode)).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    let durs = extinfs(&pl);
    let total = durs.len();
    let mut starts = vec![0.0];
    for d in &durs {
        starts.push(starts.last().unwrap() + d);
    }
    let init = std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap();
    // init: exactly ftyp + moov
    let tracks = init_tracks(&init);
    assert_eq!(
        tracks.len(),
        2,
        "only video+audio tracks (45 subtitle streams not mapped)"
    );
    let (mut has_ftyp, mut has_moov, mut p) = (false, false, 0);
    while p + 8 <= init.len() {
        let s = u32::from_be_bytes(init[p..p + 4].try_into().unwrap()) as usize;
        match &init[p + 4..p + 8] {
            b"ftyp" => has_ftyp = true,
            b"moov" => has_moov = true,
            other => panic!("unexpected box in init: {other:?}"),
        }
        p += s;
    }
    assert!(has_ftyp && has_moov && p == init.len());

    let first_resume = starts.iter().rposition(|s| *s <= 86.9).unwrap();
    let order: Vec<usize> = [
        first_resume,
        first_resume + 1,
        0,
        1,
        2,
        total - 6,
        total - 5,
        total - 4,
        3,
        first_resume,
    ]
    .into_iter()
    .map(|n| n.min(total - 1))
    .collect();
    let mut served: std::collections::BTreeMap<usize, Vec<u8>> = Default::default();
    for (i, &n) in order.iter().enumerate() {
        let seg_path = mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        let data = std::fs::read(&seg_path).unwrap();
        // structure
        let c = clean_segment(&data, &tracks)
            .unwrap_or_else(|e| panic!("segment {n} (request #{i}) invalid: {e}"));
        assert!(
            !data
                .windows(4)
                .any(|w| w == b"sidx" || w == b"ftyp" || w == b"moov"),
            "segment {n} contains init/index boxes"
        );
        assert_eq!(
            data.len(),
            c.styp.as_ref().map_or(0, |s| s.len()) + c.body.len(),
            "segment {n} is not already clean"
        );
        // fragment sequence numbers strictly increasing inside the segment
        let seq = mfhd_sequences(&data);
        assert!(
            !seq.is_empty() && seq.windows(2).all(|w| w[0] < w[1]),
            "segment {n}: sequence numbers {seq:?}"
        );
        served.insert(n, data);
    }
    // contiguity + decodability of runs of consecutive segments in playlist order
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &n in served.keys() {
        match runs.last_mut() {
            Some(r) if *r.last().unwrap() + 1 == n => r.push(n),
            _ => runs.push(vec![n]),
        }
    }
    for r in &runs {
        let segs: Vec<Vec<u8>> = r.iter().map(|n| served[n].clone()).collect();
        let (lo, hi) = probe_video(tmp.path(), &init, &segs);
        assert!(
            (lo - starts[r[0]]).abs() < 0.1,
            "run {r:?}: first pts {lo} vs plan {}",
            starts[r[0]]
        );
        assert!(
            hi <= starts[r[r.len() - 1] + 1] + 0.12,
            "run {r:?}: last pts {hi} beyond plan end {}",
            starts[r[r.len() - 1] + 1]
        );
        assert!(
            hi >= starts[r[r.len() - 1] + 1] - 0.15,
            "run {r:?}: gap at end: last pts {hi} vs plan end {}",
            starts[r[r.len() - 1] + 1]
        );
    }
    // each segment on its own after the init (what MSE sees right after a seek)
    for (n, data) in &served {
        let (lo, _) = probe_video(tmp.path(), &init, std::slice::from_ref(data));
        assert!(
            (lo - starts[*n]).abs() < 0.1,
            "segment {n} alone: {lo} vs {}",
            starts[*n]
        );
    }
    // init bytes identical to ffmpeg's current run
    let mut raw = Vec::new();
    for _ in 0..50 {
        raw = std::fs::read(dir.join("init.mp4")).unwrap_or_default();
        if reel_playback::session::init_is_complete(&raw) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(raw, init, "ffmpeg's init differs from the served init");
    mgr.shutdown().await;
}

#[tokio::test]
async fn copy_mode_segments_are_clean_across_restarts() {
    if have_ffmpeg() {
        run(false).await;
    }
}

#[tokio::test]
async fn encode_mode_segments_are_clean_across_restarts() {
    if have_ffmpeg() {
        run(true).await;
    }
}

/// A restart at segment N must not leave files of the previous run at/after N behind.
#[tokio::test]
async fn restart_removes_stale_files_from_older_runs() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.lookahead_segments = 0;
    cfg.max_ahead_segments = 3; // keeps run #1 from racing far ahead
    cfg.throttle_interval = Duration::from_millis(20);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let mut req = request(&src, false);
    req.start_position_s = 0.0;
    let info = mgr.create_session(req).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let durs = extinfs(&mgr.media_playlist_async(&info.id).await.unwrap());
    // let run #1 produce a few segments, remember them
    for n in 0..3 {
        mgr.segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    // plant a bogus "stale" file where run #2 will start and a leftover tmp
    let target = (durs.len() / 2).max(10);
    let stale = dir.join(format!("seg_{target:05}.m4s"));
    std::fs::write(&stale, b"\x47MPEG-TS-GARBAGE-NOT-FMP4").unwrap();
    std::fs::write(dir.join("seg_00099.m4s.tmp"), b"partial").unwrap();
    // requesting that segment must restart ffmpeg and never serve the garbage
    let served = mgr
        .segment_path(&info.id, &format!("seg_{target:05}.m4s"))
        .await
        .unwrap();
    let data = std::fs::read(served).unwrap();
    assert!(!data.starts_with(b"\x47MPEG"), "served stale/garbage file");
    let tracks =
        init_tracks(&std::fs::read(mgr.segment_path(&info.id, "init.mp4").await.unwrap()).unwrap());
    clean_segment(&data, &tracks).expect("valid fMP4 after restart");
    assert!(!dir.join("seg_00099.m4s.tmp").exists());
    mgr.shutdown().await;
}

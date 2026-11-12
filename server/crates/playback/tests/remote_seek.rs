//! #92: a remote (IPTV VOD) remux session must be seekable. The keyframe index is read from the
//! container metadata through the session's exclusive upstream-proxy token, so the session serves
//! a full VOD playlist (ENDLIST) and a far seek restarts ffmpeg at the right keyframe, while the
//! provider still never sees two connections at once.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use reel_playback::decision::*;
use reel_playback::model::*;
use reel_playback::upstream::UpstreamProxy;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 30 s H.264/AAC with a keyframe every second; `faststart: false` puts an MP4's `moov` at the end.
fn make_source(path: &Path, faststart: bool) {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=30",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=30"])
        .args([
            "-c:v", "libx264", "-g", "25", "-pix_fmt", "yuv420p", "-c:a", "aac",
        ]);
    if faststart {
        cmd.args(["-movflags", "+faststart"]);
    }
    assert!(cmd.arg(path).status().unwrap().success());
}

#[derive(Clone)]
struct Provider {
    file: Arc<PathBuf>,
    /// Honour `Range` (a provider that ignores it sends the whole file with 200).
    ranges: bool,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    /// Bytes actually sent.
    sent: Arc<AtomicU64>,
}

struct Open(Provider);
impl Drop for Open {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Trickles the body (16 KiB / 5 ms) and counts open connections, like a one-connection provider.
async fn serve(State(p): State<Provider>, headers: HeaderMap) -> Response {
    p.requests.fetch_add(1, Ordering::SeqCst);
    let n = p.active.fetch_add(1, Ordering::SeqCst) + 1;
    p.peak.fetch_max(n, Ordering::SeqCst);
    let guard = Open(p.clone());
    let data = bytes::Bytes::from(std::fs::read(&*p.file).unwrap());
    let len = data.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|r| r.split_once('-'))
        .filter(|_| p.ranges);
    let (a, b, status) = match range {
        Some((a, b)) => {
            let a: usize = a.parse().unwrap_or(0);
            let b: usize = b.parse().map(|b: usize| b.min(len - 1)).unwrap_or(len - 1);
            if a >= len {
                return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            }
            (a, b, StatusCode::PARTIAL_CONTENT)
        }
        None => (0, len - 1, StatusCode::OK),
    };
    let sent = p.sent.clone();
    let body = futures_util::stream::unfold(
        (guard, a, tokio::time::interval(Duration::from_millis(5))),
        move |(g, pos, mut t)| {
            let (data, sent) = (data.clone(), sent.clone());
            async move {
                if pos > b {
                    return None;
                }
                t.tick().await;
                let end = (pos + 16 * 1024).min(b + 1);
                sent.fetch_add((end - pos) as u64, Ordering::SeqCst);
                Some((Ok::<_, std::io::Error>(data.slice(pos..end)), (g, end, t)))
            }
        },
    );
    let mut r = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, b + 1 - a);
    if p.ranges {
        r = r.header(header::ACCEPT_RANGES, "bytes");
    }
    if status == StatusCode::PARTIAL_CONTENT {
        r = r.header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{len}"));
    }
    r.body(Body::from_stream(body)).unwrap()
}

async fn start_provider(file: PathBuf, ranges: bool) -> (Provider, u16) {
    let p = Provider {
        file: Arc::new(file),
        ranges,
        active: Default::default(),
        peak: Default::default(),
        requests: Default::default(),
        sent: Default::default(),
    };
    let app = Router::new()
        .route("/movie/{u}/{pw}/{name}", get(serve))
        .with_state(p.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (p, port)
}

fn remux_req(url: &str, file_id: i64, container: &str) -> StartRequest {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(320);
    v.height = Some(240);
    let mut a = StreamInfo::new(1, StreamKind::Audio, "aac");
    a.channels = Some(1);
    a.default = true;
    StartRequest {
        user_id: 1,
        device_id: None,
        file: FileInfo {
            id: file_id,
            path: url.into(),
            container: Some(container.into()),
            duration_s: Some(30.0),
            bitrate: Some(500_000),
        },
        streams: vec![v, a],
        decision: PlaybackDecision {
            mode: PlaybackMode::Remux,
            video: VideoAction::Copy,
            audio: AudioAction::Copy,
            subtitle: SubtitleAction::None,
            video_index: Some(0),
            audio_index: Some(1),
            subtitle_index: None,
            reasons: vec!["container not supported".into()],
        },
        start_position_s: 0.0,
        user_max_streams: None,
        user_max_transcodes: None,
        user_agent: Some("ProviderUA/9".into()),
    }
}

fn manager(dir: &Path) -> Arc<SessionManager> {
    let mut cfg = SessionConfig::new(dir);
    cfg.lookahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    mgr.media().set_upstream(UpstreamProxy::new(
        reqwest::Client::new(),
        Arc::new(|_| Ok(())),
    ));
    mgr
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
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "packet=pts_time", "-of", "csv=p=0"])
        .arg(&joined)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&joined);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<f64>().ok())
        .fold(f64::MAX, f64::min)
}

/// Remux a remote file, check the VOD playlist, then seek far ahead and back.
async fn seekable_remote_remux(container: &str, ext: &str, faststart: bool) {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join(format!("prov.{ext}"));
    make_source(&src, faststart);
    let (p, port) = start_provider(src, true).await;
    let mgr = manager(tmp.path());
    let url = format!("http://127.0.0.1:{port}/movie/bob/hunter2/7.{ext}");
    let info = mgr
        .create_session(remux_req(&url, 7, container))
        .await
        .unwrap();
    assert!(
        !info.fallback_playlist,
        "remote remux must not fall back to ffmpeg's live EVENT playlist"
    );
    let index_requests = p.requests.load(Ordering::SeqCst);

    // a VOD playlist: every segment listed up front, ENDLIST, no EVENT type
    let playlist = mgr.media_playlist_async(&info.id).await.unwrap();
    assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"), "{playlist}");
    assert!(playlist.contains("#EXT-X-ENDLIST"), "{playlist}");
    assert!(!playlist.contains("EVENT"), "{playlist}");
    let durs = extinfs(&playlist);
    let sum: f64 = durs.iter().sum();
    assert!((sum - 30.0).abs() < 0.05, "EXTINF sum {sum}");

    // seek: far ahead (ffmpeg restarts at that keyframe), then back to the start
    let init = mgr.segment_path(&info.id, "init.mp4").await.unwrap();
    let mut starts = vec![0.0];
    for d in &durs {
        starts.push(starts.last().unwrap() + d);
    }
    // (the source has B-frames: every run keeps the file's constant B-frame delay, so the check
    // is that all segments sit at the same offset from the playlist, whatever run produced them)
    let far = durs.len() - 2;
    let mut offsets = Vec::new();
    for n in [far, 0, far / 2] {
        let seg = mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .unwrap();
        let pts = first_video_pts(&init, &seg, tmp.path());
        offsets.push((n, pts - starts[n]));
    }
    let off0 = offsets[0].1;
    assert!(off0.abs() < 0.2, "{offsets:?}");
    assert!(
        offsets.iter().all(|(_, o)| (o - off0).abs() < 0.02),
        "segments of different ffmpeg runs disagree with the playlist: {offsets:?}"
    );
    mgr.remove(&info.id).await;
    for _ in 0..60 {
        if p.active.load(Ordering::SeqCst) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(p.active.load(Ordering::SeqCst), 0, "stop closes the input");
    assert_eq!(
        p.peak.load(Ordering::SeqCst),
        1,
        "never two provider connections at once"
    );

    // the index is cached (by a hash, never the URL): the next session reads no metadata
    let cached: Vec<String> = std::fs::read_dir(tmp.path().join("keyframes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
    assert!(cached[0].starts_with("7-remote-"), "{cached:?}");
    assert!(!cached[0].contains("hunter2") && !cached[0].contains("bob"));
    let before = p.requests.load(Ordering::SeqCst);
    let again = mgr
        .create_session(remux_req(&url, 7, container))
        .await
        .unwrap();
    assert!(!again.fallback_playlist);
    assert_eq!(
        p.requests.load(Ordering::SeqCst),
        before,
        "session creation made no provider request with a cached index"
    );
    assert!(
        index_requests <= 4,
        "{index_requests} requests to read the index"
    );
    mgr.shutdown().await;
}

#[tokio::test]
async fn remote_mkv_remux_is_seekable() {
    seekable_remote_remux("matroska,webm", "mkv", false).await;
}

#[tokio::test]
async fn remote_mp4_remux_with_moov_at_the_end_is_seekable() {
    seekable_remote_remux("mov,mp4,m4a,3gp,3g2,mj2", "mp4", false).await;
}

/// A provider that ignores `Range` would send the whole file: the index read gives up after the
/// first block (never downloads the file) and the session falls back to the EVENT playlist.
#[tokio::test]
async fn provider_without_range_support_falls_back_without_downloading_the_file() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("prov.mkv");
    make_source(&src, false);
    let size = std::fs::metadata(&src).unwrap().len();
    let (p, port) = start_provider(src, false).await;
    let mgr = manager(tmp.path());
    let url = format!("http://127.0.0.1:{port}/movie/bob/hunter2/8.mkv");
    let info = mgr
        .create_session(remux_req(&url, 8, "matroska,webm"))
        .await
        .unwrap();
    assert!(info.fallback_playlist);
    let pl = mgr.media_playlist_async(&info.id).await.unwrap();
    assert!(pl.contains("#EXTINF"), "{pl}");
    assert!(
        p.requests.load(Ordering::SeqCst) >= 2,
        "ffmpeg still plays it"
    );
    mgr.remove(&info.id).await;
    assert!(!tmp.path().join("keyframes").exists());
    assert!(p.peak.load(Ordering::SeqCst) <= 1);
    // only ffmpeg reads the body; the index attempt stopped at the 200 status line
    assert!(p.sent.load(Ordering::SeqCst) <= size + 64 * 1024);
}

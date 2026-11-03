//! Security hardening of the media/subprocess side: redaction, atomic limits, the loopback
//! upstream proxy, protocol whitelists, filter escaping/staging and scratch-disk budgets.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use reel_playback::decision::*;
use reel_playback::media::MediaCtx;
use reel_playback::model::*;
use reel_playback::subtitles::{SubtitleSource, to_vtt_with};
use reel_playback::upstream::UpstreamProxy;
use reel_playback::{HwAccel, PlaybackError, SessionConfig, SessionManager, StartRequest};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 20 s h264+aac mkv with one SRT track.
fn make_mkv(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let srt = path.with_extension("tmp.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nhello\n").unwrap();
    let st = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=20",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=20", "-i"])
        .arg(&srt)
        .args([
            "-map", "0:v", "-map", "1:a", "-map", "2:s", "-c:v", "libx264", "-g", "25", "-pix_fmt",
            "yuv420p", "-c:a", "aac", "-c:s", "srt",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(st.success());
    let _ = std::fs::remove_file(srt);
}

fn streams(with_sub: bool) -> Vec<StreamInfo> {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(320);
    v.height = Some(240);
    v.bit_depth = Some(8);
    let mut a = StreamInfo::new(1, StreamKind::Audio, "aac");
    a.channels = Some(2);
    a.default = true;
    let mut s = vec![v, a];
    if with_sub {
        s.push(StreamInfo::new(2, StreamKind::Subtitle, "subrip"));
    }
    s
}

fn remux() -> PlaybackDecision {
    PlaybackDecision {
        mode: PlaybackMode::Remux,
        video: VideoAction::Copy,
        audio: AudioAction::Copy,
        subtitle: SubtitleAction::None,
        reasons: vec![],
        video_index: Some(0),
        audio_index: Some(1),
        subtitle_index: None,
    }
}

fn transcode() -> PlaybackDecision {
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
            bitrate_kbps: 96,
        },
        ..remux()
    }
}

fn direct() -> PlaybackDecision {
    PlaybackDecision {
        mode: PlaybackMode::DirectPlay,
        video: VideoAction::Copy,
        audio: AudioAction::Copy,
        ..remux()
    }
}

fn req(path: &str, decision: PlaybackDecision, user: i64) -> StartRequest {
    StartRequest {
        user_id: user,
        device_id: None,
        file: FileInfo {
            id: user * 100 + 1,
            path: path.into(),
            container: Some("matroska,webm".into()),
            duration_s: Some(20.0),
            bitrate: Some(500_000),
        },
        streams: streams(false),
        decision,
        start_position_s: 0.0,
        user_max_streams: None,
        user_max_transcodes: None,
    }
}

/// A wrapper script that records its argv and then runs the real ffmpeg.
fn fake_ffmpeg(dir: &Path, name: &str, extra: &str) -> (String, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let args = dir.join(format!("{name}.args"));
    let sh = dir.join(format!("{name}.sh"));
    std::fs::write(
        &sh,
        format!(
            "#!/bin/sh\n{extra}\nprintf '%s\\n' \"$@\" >> '{}'\nexec ffmpeg \"$@\"\n",
            args.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    (sh.to_string_lossy().into_owned(), args)
}

// ---------------------------------------------------------------------------------------------
// provider test server (Range support, HLS, redirects)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Provider {
    file: Arc<PathBuf>,
    hits: Arc<AtomicUsize>,
    port: Arc<std::sync::atomic::AtomicU16>,
}

async fn serve_file(State(p): State<Provider>, headers: HeaderMap) -> Response {
    p.hits.fetch_add(1, Ordering::SeqCst);
    let data = std::fs::read(&*p.file).unwrap();
    let len = data.len();
    if let Some(r) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
    {
        let (a, b) = r.split_once('-').unwrap();
        let a: usize = a.parse().unwrap_or(0);
        let b: usize = b.parse().map(|b: usize| b.min(len - 1)).unwrap_or(len - 1);
        if a >= len {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{len}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .body(Body::from(data[a..=b].to_vec()))
            .unwrap();
    }
    (
        [
            (header::ACCEPT_RANGES, "bytes"),
            (header::CONTENT_TYPE, "video/x-matroska"),
        ],
        data,
    )
        .into_response()
}

async fn hls_master() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nv/index.m3u8\n",
    )
}

async fn hls_media(State(p): State<Provider>) -> impl IntoResponse {
    let port = p.port.load(Ordering::SeqCst);
    (
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"../key.bin\"\n#EXTINF:4,\nseg1.ts\n#EXTINF:4,\nhttp://127.0.0.1:{port}/hls/v/seg2.ts\n#EXT-X-ENDLIST\n"
        ),
    )
}

async fn hls_seg(AxPath(n): AxPath<String>, State(p): State<Provider>) -> impl IntoResponse {
    p.hits.fetch_add(1, Ordering::SeqCst);
    format!("SEGMENT-{n}")
}

async fn redirect() -> impl IntoResponse {
    (
        StatusCode::FOUND,
        [(header::LOCATION, "http://127.0.0.2:9/blocked")],
    )
}

async fn start_provider(file: PathBuf) -> (Provider, u16) {
    let p = Provider {
        file: Arc::new(file),
        hits: Arc::new(AtomicUsize::new(0)),
        port: Arc::new(Default::default()),
    };
    let app = Router::new()
        .route("/movie/{u}/{pw}/{name}", get(serve_file))
        .route("/media.mkv", get(serve_file))
        .route("/hls/master.m3u8", get(hls_master))
        .route("/hls/v/index.m3u8", get(hls_media))
        .route("/hls/v/{seg}", get(hls_seg))
        .route("/hls/key.bin", get(|| async { "KEYBYTES" }))
        .route("/redir", get(redirect))
        .with_state(p.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    p.port.store(port, Ordering::SeqCst);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (p, port)
}

fn open_proxy() -> UpstreamProxy {
    UpstreamProxy::new(reqwest::Client::new(), Arc::new(|_| Ok(())))
}

fn proxy_blocking_127_0_0_2() -> UpstreamProxy {
    // same shape as reel_iptv::net::client(false): redirects to blocked hosts are refused
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.url().host_str() == Some("127.0.0.2") {
                a.error("redirect to a private address is blocked")
            } else {
                a.follow()
            }
        }))
        .build()
        .unwrap();
    UpstreamProxy::new(
        client,
        Arc::new(|u| {
            if u.contains("169.254.") {
                Err("blocked".into())
            } else {
                Ok(())
            }
        }),
    )
}

async fn fetch(
    url: &str,
    range: Option<&str>,
    host: Option<&str>,
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let c = reqwest::Client::new();
    let mut r = c.get(url);
    if let Some(x) = range {
        r = r.header("Range", x);
    }
    if let Some(h) = host {
        r = r.header("Host", h);
    }
    let resp = r.send().await.unwrap();
    let status = resp.status().as_u16();
    let hs = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    (status, hs, resp.bytes().await.unwrap().to_vec())
}

// ---------------------------------------------------------------------------------------------
// 3. upstream proxy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn proxy_streams_with_range_passthrough_and_hides_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("m.bin");
    std::fs::write(&f, (0..=255u8).cycle().take(10_000).collect::<Vec<_>>()).unwrap();
    let (_p, port) = start_provider(f).await;
    let proxy = open_proxy();
    let lease = proxy
        .lease(
            &format!("http://127.0.0.1:{port}/movie/bob/hunter2/1.mkv"),
            "UA/1",
            Duration::from_secs(60),
        )
        .unwrap();
    assert!(
        lease.url().starts_with("http://127.0.0.1:"),
        "loopback only: {}",
        lease.url()
    );
    assert!(
        !lease.url().contains("hunter2")
            && !lease.url().contains(&port.to_string())
            && lease.url().ends_with("/f.mkv")
    );
    let (s, _, body) = fetch(lease.url(), None, None).await;
    assert_eq!((s, body.len()), (200, 10_000));
    let (s, h, body) = fetch(lease.url(), Some("bytes=100-199"), None).await;
    assert_eq!(s, 206);
    assert_eq!(body.len(), 100);
    assert_eq!(body[0], 100);
    assert!(
        h.iter()
            .any(|(k, v)| k == "content-range" && v == "bytes 100-199/10000"),
        "{h:?}"
    );
    let (s, _, _) = fetch(lease.url(), Some("bytes=20000-"), None).await;
    assert_eq!(s, 416);
    // a foreign Host header (DNS rebinding through a browser) gets nothing
    assert_eq!(fetch(lease.url(), None, Some("evil.example")).await.0, 404);
    // lease drop revokes
    let url = lease.url().to_string();
    drop(lease);
    assert_eq!(fetch(&url, None, None).await.0, 404);
    assert_eq!(proxy.token_count(), 0);
}

#[tokio::test]
async fn proxy_rewrites_hls_playlists_recursively() {
    let tmp = tempfile::tempdir().unwrap();
    let (prov, port) = start_provider(tmp.path().join("unused")).await;
    let proxy = open_proxy();
    let local = proxy
        .register(
            &format!("http://127.0.0.1:{port}/hls/master.m3u8"),
            "UA",
            Duration::from_secs(60),
            Some("sess"),
        )
        .unwrap();
    let (s, h, body) = fetch(&local, None, None).await;
    assert_eq!(s, 200);
    assert!(
        h.iter()
            .any(|(k, v)| k == "content-type" && v.contains("mpegurl"))
    );
    let master = String::from_utf8(body).unwrap();
    assert!(
        !master.contains(&port.to_string()) && !master.contains("hls/"),
        "{master}"
    );
    let child = master
        .lines()
        .find(|l| l.starts_with("http://127.0.0.1:"))
        .expect("child playlist URL")
        .to_string();
    let (s, _, body) = fetch(&child, None, None).await;
    assert_eq!(s, 200);
    let media = String::from_utf8(body).unwrap();
    // nested: key, relative segment and absolute segment all became proxy tokens
    assert!(
        !media.contains(&port.to_string())
            && !media.contains("key.bin")
            && !media.contains("seg1.ts"),
        "{media}"
    );
    let uris: Vec<String> = media
        .lines()
        .flat_map(|l| {
            if let Some(i) = l.find("URI=\"") {
                vec![l[i + 5..].split('"').next().unwrap().to_string()]
            } else if l.starts_with("http") {
                vec![l.to_string()]
            } else {
                vec![]
            }
        })
        .collect();
    assert_eq!(uris.len(), 3, "{media}");
    for u in &uris {
        assert!(
            u.starts_with("http://127.0.0.1:") && u.contains("/internal/upstream/"),
            "{u}"
        );
        assert_eq!(fetch(u, None, None).await.0, 200, "{u}");
    }
    let (_, _, seg) = fetch(&uris[1], None, None).await;
    assert!(String::from_utf8_lossy(&seg).starts_with("SEGMENT-"));
    assert!(prov.hits.load(Ordering::SeqCst) >= 1);
    // tokens expire with their owner
    assert!(proxy.token_count() >= 5);
    proxy.revoke_owner("sess");
    assert_eq!(proxy.token_count(), 0);
    assert_eq!(fetch(&uris[0], None, None).await.0, 404);
}

#[tokio::test]
async fn proxy_enforces_ssrf_policy_expiry_and_redirect_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let (_p, port) = start_provider(tmp.path().join("x")).await;
    let proxy = proxy_blocking_127_0_0_2();
    // policy refuses at registration (literal URL checks)…
    assert!(
        proxy
            .register(
                "http://169.254.169.254/latest/meta-data",
                "ua",
                Duration::from_secs(5),
                None
            )
            .is_err()
    );
    // …only http(s) is proxied at all
    assert!(
        proxy
            .register("file:///etc/passwd", "ua", Duration::from_secs(5), None)
            .is_err()
    );
    assert!(
        proxy
            .register("ftp://x/y", "ua", Duration::from_secs(5), None)
            .is_err()
    );
    // redirects are checked by the guarded client: the redirect target is never fetched
    let l = proxy
        .lease(
            &format!("http://127.0.0.1:{port}/redir"),
            "ua",
            Duration::from_secs(5),
        )
        .unwrap();
    let (s, _, body) = fetch(l.url(), None, None).await;
    assert_eq!(s, 502);
    assert!(!String::from_utf8_lossy(&body).contains("127.0.0.2"));
    // tokens expire
    let short = proxy
        .register(
            &format!("http://127.0.0.1:{port}/hls/key.bin"),
            "ua",
            Duration::from_millis(150),
            None,
        )
        .unwrap();
    assert_eq!(fetch(&short, None, None).await.0, 200);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(fetch(&short, None, None).await.0, 404);
    // unknown tokens
    let bogus = short
        .rsplit_once("/internal/upstream/")
        .unwrap()
        .0
        .to_string()
        + "/internal/upstream/00000000000000000000000000000000/f.bin";
    assert_eq!(fetch(&bogus, None, None).await.0, 404);
}

#[tokio::test]
async fn remote_session_never_puts_the_provider_url_in_argv_and_revokes_tokens() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("prov.mkv");
    make_mkv(&mkv);
    let (_p, port) = start_provider(mkv).await;
    let (ffmpeg, args_file) = fake_ffmpeg(tmp.path(), "ff", "");
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.ffmpeg_path = ffmpeg;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let proxy = open_proxy();
    mgr.media().set_upstream(proxy.clone());

    let provider_url =
        format!("http://bob:hunter2@127.0.0.1:{port}/movie/bob/hunter2/1.mkv?token=SECRETTOKEN");
    let info = mgr
        .create_session(req(&provider_url, remux(), 1))
        .await
        .unwrap();
    let seg = mgr.segment_path(&info.id, "seg_00000.m4s").await.unwrap();
    assert!(
        std::fs::metadata(seg).unwrap().len() > 0,
        "playback works through the proxy"
    );
    let argv = std::fs::read_to_string(&args_file).unwrap();
    for secret in ["hunter2", "bob", "SECRETTOKEN", &format!("{port}/movie")] {
        assert!(!argv.contains(secret), "argv leaks {secret}:\n{argv}");
    }
    assert!(argv.contains("/internal/upstream/"), "{argv}");
    assert!(
        argv.lines().any(|l| l == "-protocol_whitelist") && argv.lines().any(|l| l == "http,tcp"),
        "{argv}"
    );
    assert!(proxy.token_count() >= 1);
    mgr.remove(&info.id).await;
    assert_eq!(proxy.token_count(), 0, "tokens expire with the session");
    // without a proxy a remote input is refused outright (never handed to ffmpeg)
    let bare = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    assert!(
        bare.create_session(req(&provider_url, remux(), 1))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn remote_subtitles_and_probe_inputs_go_through_the_proxy() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("prov.mkv");
    make_mkv(&mkv);
    let (_p, port) = start_provider(mkv).await;
    let (ffmpeg, args_file) = fake_ffmpeg(tmp.path(), "ffsub", "");
    let media = MediaCtx::new(2);
    media.set_upstream(open_proxy());
    let src = SubtitleSource::Embedded {
        path: format!("http://u:p@127.0.0.1:{port}/movie/bob/hunter2/1.mkv"),
        stream_index: 2,
    };
    let out = to_vtt_with(&media, &ffmpeg, &src, &tmp.path().join("subs"), "remote-1")
        .await
        .unwrap();
    assert!(std::fs::read_to_string(out).unwrap().contains("hello"));
    let argv = std::fs::read_to_string(&args_file).unwrap();
    assert!(
        !argv.contains("hunter2") && !argv.contains("u:p@") && argv.contains("/internal/upstream/"),
        "{argv}"
    );
    // the legacy local-only entry point refuses remote sources
    let err = reel_playback::subtitles::to_vtt("ffmpeg", &src, &tmp.path().join("s2"), "k")
        .await
        .unwrap_err();
    assert!(matches!(err, PlaybackError::Invalid(_)));
}

// ---------------------------------------------------------------------------------------------
// 1. no raw ffmpeg stderr / credentials / paths to clients
// ---------------------------------------------------------------------------------------------

fn assert_clean(body: &str, secrets: &[&str]) {
    assert!(!body.contains("://"), "URL in response: {body}");
    assert!(
        !body.contains("/tmp") && !body.contains("/var/") && !body.contains("/private"),
        "absolute path in response: {body}"
    );
    for s in secrets {
        assert!(!body.contains(s), "{s} leaked: {body}");
    }
}

#[tokio::test]
async fn failed_responses_never_contain_urls_credentials_or_paths() {
    use axum::http::Request;
    use tower::ServiceExt;
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // 1. local file that is not media, under a revealing path
    let secret_dir = tmp.path().join("movies-SECRETDIR");
    std::fs::create_dir_all(&secret_dir).unwrap();
    let bad = secret_dir.join("not-a-video.mkv");
    std::fs::write(&bad, "this is not a matroska file at all").unwrap();
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.segment_wait_timeout = Duration::from_secs(8);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let info = mgr
        .create_session(req(bad.to_str().unwrap(), remux(), 1))
        .await
        .unwrap();
    let app = reel_playback::router::stream_router(mgr.clone());
    for uri in [
        format!("/stream/{}/main/seg_00000.m4s", info.id),
        format!("/stream/{}/main/init.mp4", info.id),
        format!("/stream/{}/main/index.m3u8", info.id),
        format!("/stream/{}/master.m3u8", info.id),
    ] {
        let r = app
            .clone()
            .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let body =
            String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
                .into_owned();
        assert_clean(
            &body,
            &["SECRETDIR", "not-a-video", tmp.path().to_str().unwrap()],
        );
        if status.is_server_error() {
            assert!(
                body.contains("ref "),
                "diagnostic reference expected: {body}"
            );
        }
    }
    // the error value itself is generic too; the redacted detail stays server side
    let err = mgr
        .segment_path(&info.id, "seg_00000.m4s")
        .await
        .unwrap_err();
    assert_clean(&err.to_string(), &["SECRETDIR"]);
    let detail = match &err {
        PlaybackError::SegmentUnavailable(d) | PlaybackError::Ffmpeg(d) => d.detail().to_string(),
        other => panic!("unexpected {other:?}"),
    };
    assert_clean(&detail, &["SECRETDIR", tmp.path().to_str().unwrap()]);
    mgr.shutdown().await;

    // 2. remote provider that returns garbage: stderr mentions the (proxy) URL → redacted
    let garbage = tmp.path().join("garbage.bin");
    std::fs::write(&garbage, vec![0u8; 4096]).unwrap();
    let (_p, port) = start_provider(garbage).await;
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.segment_wait_timeout = Duration::from_secs(8);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    mgr.media().set_upstream(open_proxy());
    let url =
        format!("http://bob:hunter2@127.0.0.1:{port}/movie/bob/hunter2/1.mkv?password=hunter2");
    let info = mgr.create_session(req(&url, remux(), 1)).await.unwrap();
    let err = mgr
        .segment_path(&info.id, "seg_00000.m4s")
        .await
        .unwrap_err();
    let shown = err.to_string();
    assert_clean(&shown, &["hunter2", "bob", &port.to_string()]);
    if let PlaybackError::SegmentUnavailable(d) | PlaybackError::Ffmpeg(d) = &err {
        assert_clean(d.detail(), &["hunter2", "bob", &port.to_string()]);
    }
    mgr.shutdown().await;
}

// ---------------------------------------------------------------------------------------------
// 2. atomic, complete media-work limits
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn user_stream_limit_is_atomic_under_concurrency() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let m = mgr.clone();
        tasks.push(tokio::spawn(async move {
            let mut r = req("/m/a.mkv", direct(), 7);
            r.user_max_streams = Some(2);
            m.create_session(r).await
        }));
    }
    let results: Vec<_> = futures_join(tasks).await;
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let limited = results
        .iter()
        .filter(|r| matches!(r, Err(PlaybackError::UserStreamLimit(2))))
        .count();
    assert_eq!(
        (ok, limited),
        (2, 14),
        "exactly the limit may pass, never more"
    );
    // other users are unaffected
    assert!(
        mgr.create_session(req("/m/a.mkv", direct(), 8))
            .await
            .is_ok()
    );
}

async fn futures_join<T: Send + 'static>(tasks: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.unwrap());
    }
    out
}

#[tokio::test]
async fn user_transcode_limit_is_atomic_and_remux_counts_toward_the_ffmpeg_budget() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("a.mkv");
    make_mkv(&mkv);
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.max_ffmpeg_processes = 3;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let p = mkv.to_str().unwrap().to_string();

    // concurrent transcodes of one user with limit 1: exactly one wins
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let (m, p) = (mgr.clone(), p.clone());
        tasks.push(tokio::spawn(async move {
            let mut r = req(&p, transcode(), 5);
            r.user_max_transcodes = Some(1);
            m.create_session(r).await
        }));
    }
    let results = futures_join(tasks).await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().filter(|r| r.is_err()).all(|r| matches!(
        r,
        Err(PlaybackError::UserLimit(1)) | Err(PlaybackError::GlobalLimit(3))
    )));
    assert_eq!(mgr.media().processes_in_use(), 1);
    mgr.shutdown().await;
    assert_eq!(
        mgr.media().processes_in_use(),
        0,
        "slots are released with the sessions"
    );

    // remux sessions use the shared process budget as well (3 slots)
    let mut ids = Vec::new();
    for u in 0..3 {
        ids.push(
            mgr.create_session(req(&p, remux(), 20 + u))
                .await
                .unwrap()
                .id,
        );
    }
    assert!(matches!(
        mgr.create_session(req(&p, remux(), 99)).await,
        Err(PlaybackError::GlobalLimit(3))
    ));
    // direct play needs no ffmpeg and is not blocked by the budget
    assert!(mgr.create_session(req(&p, direct(), 99)).await.is_ok());
    // a subtitle conversion has to wait for a slot too (bounded queue → Busy)
    let media = MediaCtx::with_queue_wait(1, Duration::from_millis(200));
    let _held = media.try_proc().unwrap();
    let src = SubtitleSource::Embedded {
        path: p.clone(),
        stream_index: 2,
    };
    let err = to_vtt_with(&media, "ffmpeg", &src, &tmp.path().join("subs"), "k1")
        .await
        .unwrap_err();
    assert!(matches!(err, PlaybackError::Busy));
    // freeing a session slot admits the next one
    mgr.remove(&ids[0]).await;
    assert!(mgr.create_session(req(&p, remux(), 99)).await.is_ok());
    mgr.shutdown().await;
}

#[tokio::test]
async fn subtitle_conversion_is_single_flight_per_cache_key() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("a.mkv");
    make_mkv(&mkv);
    let counter = tmp.path().join("runs");
    let (ffmpeg, _args) = fake_ffmpeg(
        tmp.path(),
        "ffcount",
        &format!("echo run >> '{}'; sleep 0.4", counter.display()),
    );
    let media = MediaCtx::new(4);
    let cache = tmp.path().join("subs");
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (m, f, c, p) = (
            media.clone(),
            ffmpeg.clone(),
            cache.clone(),
            mkv.to_str().unwrap().to_string(),
        );
        tasks.push(tokio::spawn(async move {
            to_vtt_with(
                &m,
                &f,
                &SubtitleSource::Embedded {
                    path: p,
                    stream_index: 2,
                },
                &c,
                "same-key",
            )
            .await
        }));
    }
    for r in futures_join(tasks).await {
        assert!(
            std::fs::read_to_string(r.unwrap())
                .unwrap()
                .contains("hello")
        );
    }
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1,
        "one conversion shared by 8 requests"
    );
    assert_eq!(media.processes_in_use(), 0);
}

#[test]
fn software_encoding_never_uses_hevc() {
    use reel_playback::ffmpeg::{BuildParams, build_args};
    let d = PlaybackDecision {
        video: VideoAction::Encode {
            codec: "hevc".into(),
            width: 1280,
            height: 720,
            bitrate_kbps: 3000,
            tonemap: false,
        },
        ..transcode()
    };
    let args = build_args(&BuildParams {
        input: "/m/a.mkv",
        decision: &d,
        hwaccel: &HwAccel::Software,
        source_video_codec: Some("hevc"),
        source_hdr: Hdr::None,
        burn_in: None,
        start_segment: 0,
        segment_secs: 4,
        out_dir: Path::new("/tmp/o"),
    })
    .join(" ");
    assert!(
        args.contains("-c:v libx264") && !args.contains("x265") && !args.contains("hevc_"),
        "{args}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. local inputs: protocol whitelist, library symlinks
// ---------------------------------------------------------------------------------------------

#[test]
fn local_inputs_are_whitelisted_to_file_only() {
    use reel_playback::ffmpeg::{BuildParams, build_args};
    let d = remux();
    let mk = |input: &str| {
        build_args(&BuildParams {
            input,
            decision: &d,
            hwaccel: &HwAccel::Software,
            source_video_codec: Some("h264"),
            source_hdr: Hdr::None,
            burn_in: None,
            start_segment: 0,
            segment_secs: 4,
            out_dir: Path::new("/tmp/o"),
        })
    };
    let local = mk("/m/a.mkv");
    let i = local
        .iter()
        .position(|a| a == "-protocol_whitelist")
        .expect("whitelist");
    assert_eq!(local[i + 1], "file");
    assert!(
        i < local.iter().position(|a| a == "-i").unwrap(),
        "must precede the input"
    );
    let remote = mk("http://127.0.0.1:1/internal/upstream/t/f.mkv");
    let i = remote
        .iter()
        .position(|a| a == "-protocol_whitelist")
        .unwrap();
    assert_eq!(remote[i + 1], "http,tcp");
    assert!(
        !remote.iter().any(|a| a == "-user_agent"),
        "the proxy supplies the user agent"
    );
}

#[tokio::test]
async fn whitelist_blocks_network_inputs_for_local_ffmpeg() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (prov, port) = start_provider(tmp.path().join("x")).await;
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file",
            "-i",
        ])
        .arg(format!("http://127.0.0.1:{port}/movie/a/b/c.mkv"))
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        prov.hits.load(Ordering::SeqCst),
        0,
        "ffmpeg must not even connect"
    );
}

// ---------------------------------------------------------------------------------------------
// 5. filter escaping + staging
// ---------------------------------------------------------------------------------------------

const NASTY: [&str; 7] = [
    "a:b.png",
    "it's.png",
    "back\\slash.png",
    "co,mma.png",
    "br[ack]ets.png",
    "semi;colon.png",
    "all: ' \\ , [ ] ;.png",
];

#[test]
fn escaped_filenames_survive_real_ffmpeg_graph_parsing() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // a source filter that takes a file name just like `subtitles=filename=`
    let has_movie = String::from_utf8_lossy(
        &Command::new("ffmpeg")
            .arg("-filters")
            .output()
            .unwrap()
            .stdout,
    )
    .contains(" movie ");
    if !has_movie {
        eprintln!("movie filter missing; skipping");
        return;
    }
    for name in NASTY {
        let p = tmp.path().join(name);
        let st = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=16x16:d=1",
                "-frames:v",
                "1",
            ])
            .arg(&p)
            .status()
            .unwrap();
        assert!(st.success(), "could not create {name}");
        let graph = format!(
            "movie=filename={},scale=8:8",
            reel_playback::ffmpeg::escape_filter_value(p.to_str().unwrap())
        );
        let out = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &graph,
                "-frames:v",
                "1",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // and an unescaped name really would break (the test is meaningful)
    let p = tmp.path().join(NASTY[3]);
    let bad = format!("movie=filename={},scale=8:8", p.display());
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &bad,
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[tokio::test]
async fn text_burn_in_is_staged_under_a_safe_name() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let nasty_dir = tmp.path().join("we:ird 'dir' [x],y;z");
    let mkv = nasty_dir.join("mo:vie 'q' [1],a;b.mkv");
    make_mkv(&mkv);
    let (ffmpeg, args_file) = fake_ffmpeg(tmp.path(), "ffburn", "");
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.ffmpeg_path = ffmpeg;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let mut r = req(mkv.to_str().unwrap(), transcode(), 1);
    r.streams = streams(true);
    r.decision.subtitle = SubtitleAction::BurnIn;
    r.decision.subtitle_index = Some(2);
    let info = mgr.create_session(r).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    let staged = dir.join("burn.ass");
    assert!(
        std::fs::read_to_string(&staged).unwrap().contains("hello"),
        "subtitle extracted under a fixed name"
    );
    // the main ffmpeg process was just spawned: wait until its argv shows up
    let mut argv = String::new();
    for _ in 0..100 {
        argv = std::fs::read_to_string(&args_file).unwrap_or_default();
        if argv.lines().any(|l| l == "-vf") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let vf: Vec<&str> = argv
        .lines()
        .skip_while(|l| *l != "-vf")
        .skip(1)
        .take(1)
        .collect();
    assert!(!vf.is_empty(), "{argv}");
    assert!(
        vf[0].contains("burn.ass") && !vf[0].contains("mo:vie") && !vf[0].contains("we:ird"),
        "{}",
        vf[0]
    );
    mgr.shutdown().await;
}

// ---------------------------------------------------------------------------------------------
// 6. scratch disk budget
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn disk_budget_evicts_idle_sessions_and_refuses_when_over() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("a.mkv");
    make_mkv(&mkv);
    let p = mkv.to_str().unwrap().to_string();
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.max_total_disk_bytes = 1; // anything written is over budget
    cfg.disk_evict_min_idle = Duration::from_millis(0);
    cfg.max_ahead_segments = 0;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let a = mgr.create_session(req(&p, remux(), 1)).await.unwrap();
    mgr.segment_path(&a.id, "seg_00000.m4s").await.unwrap();
    assert!(mgr.disk_usage().await > 1);
    // the next transcode evicts the idle session instead of failing
    let b = mgr.create_session(req(&p, remux(), 2)).await.unwrap();
    assert!(
        mgr.session(&a.id).await.is_err(),
        "oldest idle session evicted"
    );
    mgr.segment_path(&b.id, "seg_00000.m4s").await.unwrap();
    mgr.shutdown().await;

    // when nothing may be evicted (all sessions active), new transcodes get a 503-class error
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.max_total_disk_bytes = 1;
    cfg.disk_evict_min_idle = Duration::from_secs(3600);
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let a = mgr.create_session(req(&p, remux(), 1)).await.unwrap();
    mgr.segment_path(&a.id, "seg_00000.m4s").await.unwrap();
    let err = mgr.create_session(req(&p, remux(), 2)).await.unwrap_err();
    assert!(matches!(err, PlaybackError::DiskBudget), "{err:?}");
    assert!(
        mgr.session(&a.id).await.is_ok(),
        "active sessions are not evicted"
    );
    // direct play does not touch scratch space
    assert!(mgr.create_session(req(&p, direct(), 3)).await.is_ok());
    mgr.shutdown().await;
}

#[tokio::test]
async fn per_session_disk_cap_drops_segments_behind_the_playhead() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mkv = tmp.path().join("long.mkv");
    // 120 s so there are plenty of 1 s segments
    let st = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=120",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=duration=120",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-c:a",
            "aac",
        ])
        .arg(&mkv)
        .status()
        .unwrap();
    assert!(st.success());
    let mut cfg = SessionConfig::new(tmp.path());
    cfg.segment_secs = 1;
    cfg.max_session_disk_bytes = 150_000;
    cfg.throttle_interval = Duration::from_millis(20);
    cfg.max_ahead_segments = 60;
    let mgr = SessionManager::new(cfg, HwAccel::Software);
    let mut r = req(mkv.to_str().unwrap(), transcode(), 1);
    r.file.duration_s = Some(120.0);
    let info = mgr.create_session(r).await.unwrap();
    let dir = mgr.session_dir(&info.id).unwrap().unwrap();
    // the client "plays": request segments in order so the playhead moves
    let mut trimmed = false;
    for n in 0..60u32 {
        if mgr
            .segment_path(&info.id, &format!("seg_{n:05}.m4s"))
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
        if n > 25 && !dir.join("seg_00000.m4s").exists() {
            trimmed = true;
            break;
        }
    }
    assert!(
        trimmed,
        "segments far behind the playhead should have been dropped"
    );
    mgr.shutdown().await;
}

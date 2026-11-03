//! Real-browser MSE check (ignored by default): serves a session through the stream router and
//! plays it in headless Chrome with hls.js, resuming mid-file and after a seek.
//!
//! Run with:
//!   CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   HLS_JS=path/to/hls.min.js cargo test -p reel-playback --test chrome_e2e -- --ignored --nocapture

use axum::{Router, response::Html, routing::get};
use reel_playback::decision::*;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::{HwAccel, SessionConfig, SessionManager, StartRequest};
use std::path::Path;
use std::process::Command;

fn make_mkv(dir: &Path) -> std::path::PathBuf {
    let gop = std::env::var("E2E_GOP").unwrap_or_else(|_| "100".into());
    let out = dir.join("movie.mkv");
    let st = Command::new("ffmpeg")
        .args([
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
        ])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-bf",
            "2",
            "-g",
            &gop,
            "-keyint_min",
            &gop,
            "-sc_threshold",
            "0",
        ])
        .args(["-pix_fmt", "yuv420p", "-c:a", "eac3", "-b:a", "384k"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    out
}

fn request(path: &Path, start: f64) -> StartRequest {
    let mut v = StreamInfo::new(0, StreamKind::Video, "h264");
    v.width = Some(640);
    v.height = Some(272);
    v.bit_depth = Some(8);
    v.profile = Some("High".into());
    let mut a = StreamInfo::new(1, StreamKind::Audio, "eac3");
    a.channels = Some(6);
    a.default = true;
    let file = FileInfo {
        id: 291,
        path: path.to_string_lossy().into(),
        container: Some("matroska,webm".into()),
        duration_s: Some(150.0),
        bitrate: Some(1_000_000),
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
    StartRequest {
        user_id: 1,
        device_id: None,
        file,
        streams,
        decision,
        start_position_s: start,
        user_max_streams: None,
        user_max_transcodes: None,
    }
}

fn page(src: &str, start: f64, seek_to: Option<f64>) -> String {
    format!(
        r#"<!doctype html><video id=v muted playsinline></video><script src="/hls.js"></script><script>
const v=document.getElementById('v'); const errs=[];
const hls=new Hls({{startPosition:{start}}}); hls.on(Hls.Events.ERROR,(e,d)=>errs.push(d.details+(d.fatal?'!':'')));
hls.loadSource('{src}'); hls.attachMedia(v); v.muted=true; hls.on(Hls.Events.MANIFEST_PARSED,()=>v.play());
{seek}
setTimeout(()=>{{const b=v.buffered,r=[];for(let i=0;i<b.length;i++)r.push([+b.start(i).toFixed(2),+b.end(i).toFixed(2)]);
fetch('/report',{{method:'POST',body:JSON.stringify({{t:+v.currentTime.toFixed(2),rs:v.readyState,paused:v.paused,b:r,errs}})}});}},12000);
</script>"#,
        seek = seek_to
            .map(|t| format!("setTimeout(()=>{{v.currentTime={t};}},4000);"))
            .unwrap_or_default()
    )
}

async fn run_chrome(
    chrome: &str,
    url: &str,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) -> serde_json::Value {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(chrome)
        .arg(format!("--user-data-dir={}", tmp.path().display()))
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--autoplay-policy=no-user-gesture-required",
            url,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let body = tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv())
        .await
        .expect("no report from page")
        .unwrap();
    let _ = child.kill().await;
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
#[ignore]
async fn chrome_plays_after_resume_and_seek() {
    let (Ok(chrome), Ok(hls_js)) = (std::env::var("CHROME"), std::env::var("HLS_JS")) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let src = make_mkv(tmp.path());
    let mgr = SessionManager::new(SessionConfig::new(tmp.path()), HwAccel::Software);
    let hls_body = std::fs::read_to_string(hls_js).unwrap();
    let mut results = Vec::new();
    // positions on exact segment boundaries (where a late audio start leaves a hole at the
    // playhead); boundaries come from the playlist of a probe session
    let probe = mgr.create_session(request(&src, 0.0)).await.unwrap();
    let pl = mgr.media_playlist_async(&probe.id).await.unwrap();
    mgr.remove(&probe.id).await;
    let mut bounds = vec![0.0f64];
    for l in pl.lines().filter_map(|l| l.strip_prefix("#EXTINF:")) {
        bounds.push(bounds.last().unwrap() + l.trim_end_matches(',').parse::<f64>().unwrap());
    }
    let (b1, b2) = (bounds[20], bounds[24]);
    for (name, start, seek) in [
        ("resume@boundary", b1, None),
        ("start0 then seek@boundary", 0.0, Some(b2)),
        ("resume@86.9", 86.9, None),
    ] {
        let info = mgr.create_session(request(&src, start)).await.unwrap();
        let master = format!("/stream/{}/master.m3u8", info.id);
        let html = page(&master, start, seek);
        let hb = hls_body.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let app: Router = reel_playback::router::stream_router(mgr.clone())
            .route(
                "/page.html",
                get(move || {
                    let h = html.clone();
                    async move { Html(h) }
                }),
            )
            .route(
                "/report",
                axum::routing::post(move |body: String| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send(body);
                    }
                }),
            )
            .route(
                "/hls.js",
                get(move || {
                    let b = hb.clone();
                    async move { ([("content-type", "text/javascript")], b) }
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let r = run_chrome(
            &chrome,
            &format!("http://127.0.0.1:{port}/page.html"),
            &mut rx,
        )
        .await;
        eprintln!("{name}: {r}");
        results.push((name, start, seek, r));
        srv.abort();
        mgr.remove(&info.id).await;
    }
    for (name, start, seek, r) in results {
        let t = r["t"].as_f64().unwrap();
        let target = seek.unwrap_or(start);
        assert!(
            r["errs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| !e.as_str().unwrap().ends_with('!')),
            "{name}: fatal errors {r}"
        );
        assert!(
            t > target + 2.0,
            "{name}: playback stalled at {t} (target {target}): {r}"
        );
        assert!(r["rs"].as_i64().unwrap() >= 3, "{name}: readyState {r}");
        // restarts must not leave holes at segment seams (old code: [83.42,87.52],[87.59,150])
        assert_eq!(
            r["b"].as_array().unwrap().len(),
            1,
            "{name}: buffered has gaps: {r}"
        );
        assert!(
            r["errs"].as_array().unwrap().is_empty(),
            "{name}: player errors {r}"
        );
    }
}

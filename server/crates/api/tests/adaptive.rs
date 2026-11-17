//! Adaptive (multi-variant) decide through the API (#111): master playlist, start variant,
//! direct play vs remux, the kill switch and IPTV-free single-variant fallbacks.

mod common;

use std::{path::Path, process::Command, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use serde_json::{Value, json};
use tower::ServiceExt;

fn have(t: &str) -> bool {
    Command::new(t).arg("-version").output().is_ok()
}

/// 1280×720 H.264 at ~8 Mbit/s with AAC: big enough for 720p and 480p rungs.
fn make_video(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=duration=8:size=1280x720:rate=24")
        .args(["-f", "lavfi", "-i", "sine=duration=8"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-b:v",
            "8M",
            "-minrate",
            "8M",
            "-maxrate",
            "8M",
            "-bufsize",
            "2M",
            "-x264-params",
            "nal-hrd=cbr",
            "-g",
            "48",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed for {}", path.display());
}

struct Fx {
    app: Router,
    _dir: tempfile::TempDir,
}

async fn call(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn text(app: &Router, uri: &str) -> (StatusCode, String) {
    let res = call(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (s, String::from_utf8_lossy(&b).into_owned())
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = call(app, r.body(b).unwrap()).await;
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn fixture(abr: bool) -> Option<Fx> {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let mv = dir.path().join("movies");
    make_video(&mv.join("Big Remux (2022)/Big Remux (2022).mkv"));
    make_video(&mv.join("Big Direct (2022)/Big Direct (2022).mp4"));
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        abr,
        ..Default::default()
    };
    let state = AppState::new(db, cfg);
    let app = common::authed(&state, app(state.clone())).await;
    let body = json!({"name": "Movies", "kind": "movies", "paths": [mv]});
    let (s, _) = send(&app, "POST", "/api/v1/libraries", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED);
    for _ in 0..300 {
        let (_, l) = send(&app, "GET", "/api/v1/libraries", None).await;
        if l[0]["item_count"] == 2 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            return Some(Fx { app, _dir: dir });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scan timed out");
}

async fn file_id(fx: &Fx, title: &str) -> i64 {
    let q = title.replace(' ', "%20");
    let (_, s) = send(&fx.app, "GET", &format!("/api/v1/search?q={q}"), None).await;
    let id = s[0]["id"].as_i64().unwrap();
    let (_, d) = send(&fx.app, "GET", &format!("/api/v1/items/{id}"), None).await;
    d["files"][0]["id"].as_i64().unwrap()
}

async fn decide(fx: &Fx, fid: i64, extra: Value) -> Value {
    let mut body = json!({"preset": "web_chrome", "device_id": format!("dev-{fid}")});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (s, v) = send(
        &fx.app,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(body),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

fn ids(v: &Value) -> Vec<String> {
    v["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn adaptive_remux_lists_rungs_and_starts_low() {
    let Some(fx) = fixture(true).await else {
        return;
    };
    let fid = file_id(&fx, "Big Remux").await;
    let v = decide(&fx, fid, json!({"adaptive": true})).await;
    assert_eq!(v["mode"], "remux", "{v}");
    assert_eq!(v["decision"]["video"]["type"], "copy");
    assert_eq!(ids(&v), vec!["480p", "720p", "original"], "{v}");
    // no estimate: start low, then climb
    assert_eq!(v["start_variant"], "480p");
    assert_eq!(v["quality"], "original");
    let r720 = &v["variants"][1];
    assert_eq!(r720["transcode"], true);
    assert_eq!(
        (r720["width"].as_u64(), r720["height"].as_u64()),
        (Some(1280), Some(720))
    );
    assert!(
        r720["codecs"].as_str().unwrap().starts_with("avc1.64"),
        "{r720}"
    );
    assert_eq!(v["variants"][2]["transcode"], false);

    let sid = v["session_id"].as_str().unwrap();
    let (s, m) = text(&fx.app, v["url"].as_str().unwrap()).await;
    assert_eq!(s, 200);
    assert_eq!(m.matches("#EXT-X-STREAM-INF").count(), 3, "{m}");
    let (a, b, c) = (
        m.find("480p/index.m3u8").unwrap(),
        m.find("720p/index.m3u8").unwrap(),
        m.find("original/index.m3u8").unwrap(),
    );
    assert!(a < b && b < c, "ascending bandwidth: {m}");
    assert!(
        m.contains("AVERAGE-BANDWIDTH=") && m.contains("CODECS=\"avc1."),
        "{m}"
    );
    // the start variant was prestarted; its playlist and first segment are served
    let base = format!("/api/v1/stream/{sid}");
    let (s, pl) = text(&fx.app, &format!("{base}/480p/index.m3u8")).await;
    assert_eq!(s, 200);
    assert!(pl.contains("#EXTINF"));
    let (s, _) = text(&fx.app, &format!("{base}/480p/seg_00000.m4s")).await;
    assert_eq!(s, 200);
    // every variant serves the same playlist (shared plan)
    let (_, pl_orig) = text(&fx.app, &format!("{base}/original/index.m3u8")).await;
    assert_eq!(pl, pl_orig);
    // Now playing shows the variants and what is delivered
    let (_, a) = send(&fx.app, "GET", "/api/v1/admin/sessions", None).await;
    let me = a
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == sid)
        .unwrap();
    assert_eq!(me["delivering"], "480p", "{me}");
    let states: Vec<&str> = me["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["state"].as_str().unwrap())
        .collect();
    assert_eq!(states.len(), 3, "{me}");
    // unknown variants are 404
    let (s, _) = text(&fx.app, &format!("{base}/1080p/index.m3u8")).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn estimate_picks_the_start_and_direct_play_when_it_fits() {
    let Some(fx) = fixture(true).await else {
        return;
    };
    // a fast link starts on the original
    let remux = file_id(&fx, "Big Remux").await;
    let v = decide(
        &fx,
        remux,
        json!({"adaptive": true, "bandwidth_kbps": 60000}),
    )
    .await;
    assert_eq!(v["start_variant"], "original", "{v}");
    // an explicit quality caps the start
    let v = decide(
        &fx,
        remux,
        json!({"adaptive": true, "bandwidth_kbps": 60000, "quality": "480p"}),
    )
    .await;
    assert_eq!(v["start_variant"], "480p", "{v}");
    assert_eq!(
        ids(&v).len(),
        3,
        "the master still lists every variant: {v}"
    );

    // a direct-playable MP4: direct play when the link carries 1.5× the file ...
    let direct = file_id(&fx, "Big Direct").await;
    let v = decide(
        &fx,
        direct,
        json!({"adaptive": true, "bandwidth_kbps": 60000}),
    )
    .await;
    assert_eq!(v["mode"], "direct_play", "{v}");
    assert!(v["variants"].as_array().unwrap().is_empty());
    assert!(v.get("start_variant").is_none());
    // ... else a remux original with rungs (also on a first play with no estimate)
    for extra in [
        json!({"adaptive": true, "bandwidth_kbps": 5000}),
        json!({"adaptive": true}),
    ] {
        let v = decide(&fx, direct, extra).await;
        assert_eq!(v["mode"], "remux", "{v}");
        assert_eq!(ids(&v), vec!["480p", "720p", "original"], "{v}");
    }
    // a non-adaptive player is unchanged
    let v = decide(&fx, direct, json!({})).await;
    assert_eq!(v["mode"], "direct_play");
    let v = decide(&fx, remux, json!({})).await;
    assert_eq!(ids(&v), vec!["original"], "{v}");
    assert!(v.get("start_variant").is_none());
    let (_, m) = text(&fx.app, v["url"].as_str().unwrap()).await;
    assert!(m.contains("\nmain/index.m3u8\n"), "{m}");
}

#[tokio::test]
async fn kill_switch_serves_one_rendition() {
    let Some(fx) = fixture(false).await else {
        return;
    };
    let fid = file_id(&fx, "Big Remux").await;
    let v = decide(&fx, fid, json!({"adaptive": true})).await;
    assert_eq!(v["mode"], "remux");
    assert_eq!(ids(&v), vec!["original"], "{v}");
    let (_, m) = text(&fx.app, v["url"].as_str().unwrap()).await;
    assert_eq!(m.matches("#EXT-X-STREAM-INF").count(), 1, "{m}");
    // casts never get several variants either (checked with ABR on in the other tests)
}

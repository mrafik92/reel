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

fn make_video(path: &Path, vcodec: &str, acodec: &str, secs: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=duration={secs}:size=128x96:rate=10"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=duration={secs}"))
        .args(["-c:v", vcodec, "-pix_fmt", "yuv420p", "-c:a", acodec])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed for {}", path.display());
}

struct Fx {
    app: Router,
    _dir: tempfile::TempDir,
    pool: sqlx::SqlitePool,
}

async fn call(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn body_bytes(res: Response<Body>) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
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
    let b = body_bytes(res).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, "GET", uri, None).await
}

async fn fixture() -> Option<Fx> {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return None;
    }
    let enc = Command::new("ffmpeg").arg("-encoders").output().unwrap();
    let enc = String::from_utf8_lossy(&enc.stdout);
    if !enc.contains("libx264") || !enc.contains(" ac3 ") {
        eprintln!("libx264/ac3 encoders missing; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let mv = dir.path().join("movies");
    make_video(
        &mv.join("Chrome Ok (2020)/Chrome Ok (2020).mp4"),
        "libx264",
        "aac",
        3,
    );
    std::fs::write(
        mv.join("Chrome Ok (2020)/Chrome Ok (2020).en.srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nHello there\n\n2\n00:00:01,500 --> 00:00:02,500\nSecond line\n",
    )
    .unwrap();
    make_video(
        &mv.join("Needs Audio (2021)/Needs Audio (2021).mkv"),
        "libx264",
        "ac3",
        6,
    );
    let tv = dir.path().join("tv");
    for e in 1..=2 {
        make_video(
            &tv.join(format!("Show (2010)/Season 01/Show - S01E0{e}.mp4")),
            "libx264",
            "aac",
            2,
        );
    }
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        max_user_transcodes: 1,
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let app = common::authed(&state, app(state.clone())).await;
    for (name, kind, p) in [("Movies", "movies", &mv), ("TV", "shows", &tv)] {
        let body = json!({"name": name, "kind": kind, "paths": [p]});
        let (s, _) = send(&app, "POST", "/api/v1/libraries", Some(body)).await;
        assert_eq!(s, StatusCode::CREATED);
    }
    for _ in 0..300 {
        let (_, l) = get(&app, "/api/v1/libraries").await;
        if l[0]["item_count"] == 2 && l[1]["item_count"] == 1 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            return Some(Fx {
                app,
                _dir: dir,
                pool: db,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scan timed out");
}

async fn movie(fx: &Fx, title: &str) -> (i64, i64) {
    let q = title.replace(' ', "%20");
    let (_, s) = get(&fx.app, &format!("/api/v1/search?q={q}")).await;
    let id = s[0]["id"].as_i64().unwrap();
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{id}")).await;
    (id, d["files"][0]["id"].as_i64().unwrap())
}

fn decide_uri(fid: i64) -> String {
    format!("/api/v1/playback/{fid}/decide")
}

#[tokio::test]
async fn decide_direct_play_chrome() {
    let Some(fx) = fixture().await else { return };
    let (item, fid) = movie(&fx, "Chrome Ok").await;
    let chrome = Some(json!({"preset":"web_chrome"}));
    let (s, v) = send(&fx.app, "POST", &decide_uri(fid), chrome).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["mode"], "direct_play");
    assert!(
        v["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("/api/v1/direct/{fid}?exp=")),
        "{v}"
    );
    assert!(v["decision"]["reasons"].is_array());
    assert_eq!(v["audio"][0]["codec"], "aac");
    let sub = &v["subtitles"][0];
    assert_eq!(sub["language"], "en");
    assert_eq!(sub["external"], true);
    let url = sub["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/v1/subs/") && url.contains(".vtt?exp="));

    // vtt endpoint on the sidecar srt
    let res = call(&fx.app, Request::get(&url).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 200);
    let ct = res.headers()[header::CONTENT_TYPE].to_str().unwrap();
    assert!(ct.starts_with("text/vtt"));
    let text = String::from_utf8(body_bytes(res).await).unwrap();
    assert!(text.starts_with("WEBVTT"), "{text}");
    assert!(text.contains("Hello there"));
    assert_eq!(get(&fx.app, "/api/v1/subs/99999.vtt").await.0, 404);

    // DB row persisted; unknown file / preset
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM playback_sessions WHERE mode='direct'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let st = send(&fx.app, "POST", &decide_uri(99999), Some(json!({})))
        .await
        .0;
    assert_eq!(st, 404);
    let bad = Some(json!({"preset":"nope"}));
    assert_eq!(send(&fx.app, "POST", &decide_uri(fid), bad).await.0, 400);

    // progress -> watch_state + continue watching, then played at >= 90%
    let sid = v["session_id"].as_str().unwrap();
    let prog = format!("/api/v1/playback/{sid}/progress");
    let (s, _) = send(
        &fx.app,
        "POST",
        &prog,
        Some(json!({"position_s":1.5,"paused":false})),
    )
    .await;
    assert_eq!(s, 204);
    let (_, h) = get(&fx.app, "/api/v1/home").await;
    assert_eq!(h["continue_watching"][0]["id"], item);
    let p = h["continue_watching"][0]["progress"].as_f64().unwrap();
    assert!(p > 0.3 && p < 0.7, "{p}");
    let (s, _) = send(
        &fx.app,
        "POST",
        &prog,
        Some(json!({"position_s":2.9,"paused":true})),
    )
    .await;
    assert_eq!(s, 204);
    let (_, h) = get(&fx.app, "/api/v1/home").await;
    assert_eq!(h["continue_watching"], json!([]));
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{item}")).await;
    assert_eq!(d["played"], true);
    let (pc, lp): (i64, Option<i64>) =
        sqlx::query_as("SELECT play_count, last_played_at FROM watch_state WHERE item_id = ?")
            .bind(item)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(pc, 1);
    assert!(lp.is_some());

    // admin list, then delete
    let (_, a) = get(&fx.app, "/api/v1/admin/sessions").await;
    assert_eq!(a[0]["item_title"], "Chrome Ok");
    assert_eq!(a[0]["mode"], "direct_play");
    assert_eq!(a[0]["username"], "admin");
    let del = format!("/api/v1/playback/{sid}");
    assert_eq!(send(&fx.app, "DELETE", &del, None).await.0, 204);
    assert_eq!(send(&fx.app, "DELETE", &del, None).await.0, 404);
    let again = Some(json!({"position_s":1}));
    assert_eq!(send(&fx.app, "POST", &prog, again).await.0, 404);
    let (_, a) = get(&fx.app, "/api/v1/admin/sessions").await;
    assert_eq!(a, json!([]));
}

#[tokio::test]
async fn decide_audio_transcode_serves_playlist_and_limits() {
    let Some(fx) = fixture().await else { return };
    let (_, fid) = movie(&fx, "Needs Audio").await;
    let chrome = || Some(json!({"preset":"web_chrome"}));
    let (s, v) = send(&fx.app, "POST", &decide_uri(fid), chrome()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["mode"], "audio_transcode", "{v}");
    let sid = v["session_id"].as_str().unwrap();
    let url = v["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("/api/v1/stream/{sid}/master.m3u8?exp=")));
    let res = call(&fx.app, Request::get(url).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    let text = String::from_utf8(body_bytes(res).await).unwrap();
    assert!(text.starts_with("#EXTM3U"));
    let idx = format!("/api/v1/stream/{sid}/main/index.m3u8");
    let res = call(&fx.app, Request::get(idx).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 200);
    assert!(
        String::from_utf8(body_bytes(res).await)
            .unwrap()
            .contains("#EXTINF")
    );

    let (_, a) = get(&fx.app, "/api/v1/admin/sessions").await;
    assert_eq!(a[0]["mode"], "audio_transcode");
    assert!(!a[0]["reasons"].as_array().unwrap().is_empty());

    // per-user limit (1) -> 429 with a message
    let (s, v) = send(&fx.app, "POST", &decide_uri(fid), chrome()).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(v["error"].as_str().unwrap().contains("limit"));

    // admin kill frees the slot
    let kill = format!("/api/v1/admin/sessions/{sid}");
    assert_eq!(send(&fx.app, "DELETE", &kill, None).await.0, 204);
    let (s, _) = send(&fx.app, "POST", &decide_uri(fid), chrome()).await;
    assert_eq!(s, 200);

    let (s, hw) = get(&fx.app, "/api/v1/admin/hwaccel").await;
    assert_eq!(s, 200);
    assert_eq!(hw["selected"]["type"], "software");
}

#[tokio::test]
async fn played_toggle_cascades() {
    let Some(fx) = fixture().await else { return };
    let (_, shows) = get(&fx.app, "/api/v1/libraries/2/items").await;
    let show = shows["items"][0]["id"].as_i64().unwrap();
    assert_eq!(shows["items"][0]["played"], false);
    let (_, seasons) = get(&fx.app, &format!("/api/v1/items/{show}/children")).await;
    let season = seasons[0]["id"].as_i64().unwrap();
    let played = |id: i64| format!("/api/v1/items/{id}/played");

    assert_eq!(send(&fx.app, "POST", &played(show), None).await.0, 204);
    let (_, eps) = get(&fx.app, &format!("/api/v1/items/{season}/children")).await;
    assert!(eps.as_array().unwrap().iter().all(|e| e["played"] == true));
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{show}")).await;
    assert_eq!(d["played"], true);
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{season}")).await;
    assert_eq!(d["played"], true);

    // unmark one episode -> show no longer fully played
    let ep = eps[0]["id"].as_i64().unwrap();
    assert_eq!(send(&fx.app, "DELETE", &played(ep), None).await.0, 204);
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{show}")).await;
    assert_eq!(d["played"], false);

    assert_eq!(send(&fx.app, "DELETE", &played(season), None).await.0, 204);
    let (_, eps) = get(&fx.app, &format!("/api/v1/items/{season}/children")).await;
    assert!(eps.as_array().unwrap().iter().all(|e| e["played"] == false));
    assert_eq!(send(&fx.app, "POST", &played(99999), None).await.0, 404);

    let (item, _) = movie(&fx, "Chrome Ok").await;
    send(&fx.app, "POST", &played(item), None).await;
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{item}")).await;
    assert_eq!(d["played"], true);
}

#[tokio::test]
async fn decide_reports_quality_and_redecide_replaces_the_session() {
    let Some(fx) = fixture().await else { return };
    let (_, fid) = movie(&fx, "Chrome Ok").await;
    let body =
        json!({"preset":"web_chrome","device_id":"dev-1","quality":"auto","max_bitrate_kbps":3500});
    let (s, v) = send(&fx.app, "POST", &decide_uri(fid), Some(body.clone())).await;
    assert_eq!(s, 200, "{v}");
    // a tiny file fits any bandwidth: original, with a menu that never offers upscaling
    assert_eq!(v["quality"], "original");
    assert!(v["bitrate_kbps"].as_u64().is_some());
    let ids: Vec<&str> = v["quality_options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["auto", "original", "1080p", "720p", "480p"]);
    assert_eq!(
        v["quality_options"][2]["available"], false,
        "128x96 source: no upscaling"
    );
    let first = v["session_id"].as_str().unwrap().to_string();

    // unknown quality id is rejected
    let bad = json!({"preset":"web_chrome","quality":"4k"});
    assert_eq!(
        send(&fx.app, "POST", &decide_uri(fid), Some(bad)).await.0,
        422
    );

    // same device + file again: the first session is replaced, not accumulated
    let (s, v2) = send(&fx.app, "POST", &decide_uri(fid), Some(body)).await;
    assert_eq!(s, 200);
    assert_ne!(v2["session_id"], first.as_str());
    let (_, a) = get(&fx.app, "/api/v1/admin/sessions").await;
    assert_eq!(a.as_array().unwrap().len(), 1, "{a}");
    let prog = format!("/api/v1/playback/{first}/progress");
    assert_eq!(
        send(&fx.app, "POST", &prog, Some(json!({"position_s":1})))
            .await
            .0,
        404
    );
}

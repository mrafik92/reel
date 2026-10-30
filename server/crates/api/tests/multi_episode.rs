mod common;

use std::{process::Command, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use serde_json::{Value, json};
use tower::ServiceExt;

fn send(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    let app = app.clone();
    let mut r = Request::builder().method(m).uri(uri);
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let req = r.body(b).unwrap();
    async move {
        let res = app.oneshot(req).await.unwrap();
        let s = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

fn get(app: &Router, uri: &str) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    send(app, "GET", uri, None)
}

#[tokio::test]
async fn multi_episode_file_is_listed_per_episode_and_progress_goes_to_the_watched_one() {
    let have = |t: &str| Command::new(t).arg("-version").output().is_ok();
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg missing; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let shows = dir.path().join("shows");
    let f = shows.join("Duo (2020)/Season 01/Duo - S01E02E03.mp4");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=4:size=128x96:rate=10"
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=duration=4",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac"
            ])
            .arg(&f)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "needs libx264"
    );
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let app = common::authed(&state, app(state.clone())).await;
    let (s, lib) = send(
        &app,
        "POST",
        "/api/v1/libraries",
        Some(json!({"name":"TV","kind":"shows","paths":[shows]})),
    )
    .await;
    assert_eq!(s, 201);
    let lib_id = lib["id"].as_i64().unwrap();
    for _ in 0..200 {
        if get(&app, "/api/v1/libraries").await.1[0]["item_count"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (_, shows_page) = get(&app, &format!("/api/v1/libraries/{lib_id}/items")).await;
    let show = shows_page["items"][0]["id"].as_i64().unwrap();
    let (_, seasons) = get(&app, &format!("/api/v1/items/{show}/children")).await;
    let (_, eps) = get(
        &app,
        &format!("/api/v1/items/{}/children", seasons[0]["id"]),
    )
    .await;
    let nos: Vec<i64> = eps
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["episode_no"].as_i64().unwrap())
        .collect();
    assert_eq!(nos, [2, 3], "both episodes are listed");
    let (e2, e3) = (
        eps[0]["id"].as_i64().unwrap(),
        eps[1]["id"].as_i64().unwrap(),
    );

    // both details expose the same file; item_ids tell the client about the sharing
    let (_, d2) = get(&app, &format!("/api/v1/items/{e2}")).await;
    let (_, d3) = get(&app, &format!("/api/v1/items/{e3}")).await;
    assert_eq!(d2["files"][0]["id"], d3["files"][0]["id"]);
    assert_eq!(d3["files"][0]["item_ids"], json!([e2, e3]));
    assert_eq!(d3["files"].as_array().unwrap().len(), 1);
    let fid = d3["files"][0]["id"].as_i64().unwrap();
    assert!(d3["files"][0]["duration_s"].as_f64().unwrap() > 3.0);

    // playing episode 3 starts the file; progress is credited to episode 3
    let decide = |body: Value| {
        send(
            &app,
            "POST",
            &format!("/api/v1/playback/{fid}/decide"),
            Some(body),
        )
    };
    assert_eq!(
        decide(json!({"preset":"web_chrome","item_id": show}))
            .await
            .0,
        400,
        "not part of the file"
    );
    let (s, v) = decide(json!({"preset":"web_chrome","item_id": e3})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["mode"], "direct_play");
    let sid = v["session_id"].as_str().unwrap();
    let prog = |p: f64| {
        send(
            &app,
            "POST",
            &format!("/api/v1/playback/{sid}/progress"),
            Some(json!({"position_s": p})),
        )
    };
    assert_eq!(prog(1.5).await.0, 204);
    let (_, home) = get(&app, "/api/v1/home").await;
    assert_eq!(home["continue_watching"].as_array().unwrap().len(), 1);
    assert_eq!(home["continue_watching"][0]["id"], e3);
    let p = home["continue_watching"][0]["progress"].as_f64().unwrap();
    assert!(p > 0.2 && p < 0.6, "progress relative to the file: {p}");
    assert_eq!(
        get(&app, &format!("/api/v1/items/{e2}")).await.1["played"],
        false
    );

    // finishing the file marks every episode in it played
    assert_eq!(prog(3.9).await.0, 204);
    assert_eq!(
        get(&app, &format!("/api/v1/items/{e2}")).await.1["played"],
        true
    );
    assert_eq!(
        get(&app, &format!("/api/v1/items/{e3}")).await.1["played"],
        true
    );
    assert_eq!(
        get(&app, "/api/v1/home").await.1["continue_watching"],
        json!([])
    );
    // default (no item_id) credits the first episode
    let (_, v) = decide(json!({"preset":"web_chrome"})).await;
    let sid2 = v["session_id"].as_str().unwrap();
    let item: i64 =
        sqlx::query_scalar("SELECT item_id FROM playback_sessions WHERE session_key = ?")
            .bind(sid2)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(item, e2);
}

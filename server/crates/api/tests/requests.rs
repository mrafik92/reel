mod common;

use std::{path::Path, process::Command, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_metadata::TmdbClient;
use reel_requests::{Options, RequestsService, TrackerConfig, notify::RetryPolicy};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

async fn send(app: &Router, m: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(m).uri(uri);
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let s = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, "GET", uri, None).await
}

fn fast_options() -> Options {
    Options {
        tracker: TrackerConfig {
            poll_interval: Duration::from_millis(25),
            stall_after: Duration::from_secs(600),
            search_grace: Duration::from_secs(600),
            emit_throttle: Duration::ZERO,
            idle_recheck: Duration::from_secs(30),
            queue_page_size: 100,
        },
        notify_retry: RetryPolicy {
            attempts: 1,
            base_delay: Duration::from_millis(1),
        },
    }
}

struct Env {
    state: AppState,
    app: Router,
    dir: tempfile::TempDir,
}

async fn env(tmdb: &MockServer, tracker: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        webhook_secret: Some("s3cret".into()),
        ..Default::default()
    };
    let mut state = AppState::new(db.clone(), cfg);
    let client = TmdbClient::with_base_url("k", tmdb.uri());
    state = if tracker {
        let svc =
            RequestsService::with_options(db, Some(client), state.events.clone(), fast_options());
        svc.spawn_tracker();
        state.with_requests(svc)
    } else {
        state.with_tmdb(client)
    };
    let app = common::authed(&state, app(state.clone())).await;
    Env { state, app, dir }
}

async fn mock_tmdb_movie(s: &MockServer, id: i64, title: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/movie/{id}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": id, "title": title, "poster_path": "/p.jpg"})),
        )
        .mount(s)
        .await;
}

async fn mock_radarr(s: &MockServer, tmdb: i64, movie_id: i64) {
    Mock::given(method("GET"))
        .and(path("/api/v3/system/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"appName":"Radarr"})))
        .mount(s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .and(query_param("tmdbId", tmdb.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie/lookup/tmdb"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"title":"The Matrix","tmdbId":tmdb,"year":1999,"images":[]})),
        )
        .mount(s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"id":movie_id,"title":"The Matrix","tmdbId":tmdb,"monitored":true,"hasFile":false}),
        ))
        .expect(1)
        .mount(s)
        .await;
}

async fn add_radarr(app: &Router, url: &str) -> i64 {
    let (s, v) = send(
        app,
        "POST",
        "/api/v1/admin/arr-servers",
        Some(
            json!({"kind":"radarr","name":"Radarr","url":url,"api_key":"supersecretkey",
                    "default_profile":4,"default_root":"/data/root"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_i64().unwrap()
}

fn empty_page() -> Value {
    json!({"page":1,"results":[],"total_pages":0,"total_results":0})
}

#[tokio::test]
async fn discover_search_shows_availability_and_progress() {
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page":1,"total_pages":1,"total_results":3,
            "results":[
              {"id":603,"title":"The Matrix","release_date":"1999-03-30","poster_path":"/m.jpg","popularity":90.0,"vote_average":8.2},
              {"id":604,"title":"The Matrix Reloaded","release_date":"2003-05-15","popularity":50.0},
              {"id":605,"title":"The Matrix Revolutions","release_date":"2003-11-05","popularity":40.0}
            ]})))
        .mount(&tmdb)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_page()))
        .mount(&tmdb)
        .await;
    let e = env(&tmdb, false).await;
    // 603 is in the library (with a file); 605 has an in-flight request.
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1, 'M', 'movies')")
        .execute(&e.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (1, 1, 'movie', 'The Matrix', 603)")
        .execute(&e.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (1, '/x.mkv', 1, 1)")
        .execute(&e.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO requests (user_id, media_type, tmdb_id, status) VALUES (1, 'movie', 605, 'downloading')")
        .execute(&e.state.db)
        .await
        .unwrap();

    let (s, v) = get(&e.app, "/api/v1/discover/search?q=matrix").await;
    assert_eq!(s, 200, "{v}");
    let by_id = |id: i64| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|c| c["tmdb_id"] == id)
            .unwrap()
    };
    assert_eq!(by_id(603)["status"], "available");
    assert_eq!(by_id(604)["status"], "none");
    assert_eq!(by_id(605)["status"], "requested");
    assert_eq!(by_id(603)["media_type"], "movie");
    assert_eq!(by_id(603)["year"], 1999);
    assert!(
        by_id(603)["poster_url"]
            .as_str()
            .unwrap()
            .starts_with("/api/v1/images/tmdb/poster?path=%2Fm.jpg&w=342&exp=")
    );
    assert!(by_id(604).get("poster_url").is_none_or(Value::is_null));

    assert_eq!(get(&e.app, "/api/v1/discover/search?q=").await.1, json!([]));
    assert_eq!(get(&e.app, "/api/v1/discover/bogus").await.0, 404);
    assert_eq!(get(&e.app, "/api/v1/discover/popular?media=x").await.0, 400);
    // TMDB image proxy validates its input.
    assert_eq!(
        get(&e.app, "/api/v1/images/tmdb/poster?path=..%2Fetc")
            .await
            .0,
        400
    );
}

#[tokio::test]
async fn discover_without_tmdb_is_424() {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let state = AppState::new(db, cfg);
    let app = common::authed(&state, app(state.clone())).await;
    let (s, v) = get(&app, "/api/v1/discover/search?q=x").await;
    assert_eq!(s, 424);
    assert!(v["error"].as_str().unwrap().contains("TMDB"));
}

#[tokio::test]
async fn create_request_pushes_to_radarr_and_is_active() {
    let tmdb = MockServer::start().await;
    mock_tmdb_movie(&tmdb, 603, "The Matrix").await;
    let radarr = MockServer::start().await;
    mock_radarr(&radarr, 603, 42).await;
    let e = env(&tmdb, false).await;

    let body = json!({"media_type":"movie","tmdb_id":603});

    let sid = add_radarr(&e.app, &radarr.uri()).await;
    // key masked, never returned
    let (_, list) = get(&e.app, "/api/v1/admin/arr-servers").await;
    assert_eq!(list[0]["api_key_hint"], "••••tkey");
    assert!(!list.to_string().contains("supersecretkey"));

    let (s, r) = send(&e.app, "POST", "/api/v1/requests", Some(body.clone())).await;
    assert_eq!(s, 201, "{r}");
    assert_eq!(r["status"], "searching");
    assert_eq!(r["arr_server_id"], sid);
    assert_eq!(r["title"], "The Matrix");
    assert_eq!(r["media_type"], "movie");
    assert!(
        r["poster_url"]
            .as_str()
            .unwrap()
            .starts_with("/api/v1/images/tmdb/poster?path=%2Fp.jpg&w=342&exp=")
    );

    let (_, active) = get(&e.app, "/api/v1/requests/active").await;
    assert_eq!(active[0]["state"], "searching");
    assert_eq!(active[0]["tmdb_id"], 603);
    assert_eq!(active[0]["request_id"], r["id"]);
    // duplicate
    assert_eq!(
        send(&e.app, "POST", "/api/v1/requests", Some(body)).await.0,
        409
    );
    let (_, l) = get(&e.app, "/api/v1/requests?status=searching").await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert_eq!(
        get(&e.app, "/api/v1/requests?status=available").await.1,
        json!([])
    );
    // retry on a searching request re-searches; approve of a non-pending one conflicts
    assert_eq!(
        send(
            &e.app,
            "POST",
            &format!("/api/v1/requests/{}/approve", r["id"]),
            None
        )
        .await
        .0,
        409
    );
    assert_eq!(
        send(&e.app, "POST", "/api/v1/requests/999/approve", None)
            .await
            .0,
        404
    );

    // the radarr add was called exactly once (verified on drop via expect(1))
    // profiles endpoint
    Mock::given(method("GET"))
        .and(path("/api/v3/qualityprofile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id":4,"name":"HD-1080p"}])))
        .mount(&radarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/rootfolder"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id":1,"path":"/data/root","freeSpace":12345}])),
        )
        .mount(&radarr)
        .await;
    let (s, p) = get(&e.app, &format!("/api/v1/admin/arr-servers/{sid}/profiles")).await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["quality_profiles"][0]["name"], "HD-1080p");
    assert_eq!(p["root_folders"][0]["free_space"], 12345);

    // PATCH keeps key, changes name; bad key is rejected; DELETE
    let (s, v) = send(
        &e.app,
        "PATCH",
        &format!("/api/v1/admin/arr-servers/{sid}"),
        Some(json!({"name":"Main","is_4k":true})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["name"], "Main");
    assert_eq!(v["is_4k"], true);
    assert_eq!(v["default_profile"], 4);
    assert_eq!(
        send(
            &e.app,
            "DELETE",
            &format!("/api/v1/admin/arr-servers/{sid}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(
            &e.app,
            "DELETE",
            &format!("/api/v1/admin/arr-servers/{sid}"),
            None
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn arr_server_validation_errors() {
    let tmdb = MockServer::start().await;
    let e = env(&tmdb, false).await;
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/system/status"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&bad)
        .await;
    let post = |body: Value| send(&e.app, "POST", "/api/v1/admin/arr-servers", Some(body));
    let (s, v) = post(json!({"kind":"radarr","name":"x","url":bad.uri(),"api_key":"k"})).await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("API key"));
    let (s, _) =
        post(json!({"kind":"radarr","name":"x","url":"http://127.0.0.1:1","api_key":"k"})).await;
    assert_eq!(s, 400);
    let (s, _) = post(json!({"kind":"plex","name":"x","url":bad.uri(),"api_key":"k"})).await;
    assert_eq!(s, 400);
    // flavour mismatch
    let sonarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/system/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"appName":"Sonarr"})))
        .mount(&sonarr)
        .await;
    let (s, v) = post(json!({"kind":"radarr","name":"x","url":sonarr.uri(),"api_key":"k"})).await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("Sonarr"));
    assert_eq!(get(&e.app, "/api/v1/admin/arr-servers").await.1, json!([]));
}

#[tokio::test]
async fn progress_events_over_websocket_to_importing() {
    let tmdb = MockServer::start().await;
    mock_tmdb_movie(&tmdb, 603, "The Matrix").await;
    let radarr = MockServer::start().await;
    mock_radarr(&radarr, 603, 42).await;
    Mock::given(method("GET"))
        .and(path("/api/v3/command"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&radarr)
        .await;
    let item = |left: f64, status: &str, tds: &str| {
        json!({"page":1,"pageSize":100,"totalRecords":1,"records":[{
            "id":1,"movieId":42,"title":"Matrix.1999.1080p","size":1000.0,"sizeleft":left,
            "timeleft":"00:09:00","status":status,"trackedDownloadState":tds,
            "trackedDownloadStatus":"ok","downloadId":"ABC123"}]})
    };
    let empty = json!({"page":1,"pageSize":100,"totalRecords":0,"records":[]});
    for body in [
        empty.clone(),
        item(1000.0, "queued", "downloading"),
        item(500.0, "downloading", "downloading"),
        item(0.0, "completed", "importPending"),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/v3/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(&radarr)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty))
        .mount(&radarr)
        .await;

    let e = env(&tmdb, true).await;
    add_radarr(&e.app, &radarr.uri()).await;

    // Real server so we can speak WebSocket.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = e.app.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events"))
        .await
        .unwrap();

    let (s, r) = send(
        &e.app,
        "POST",
        "/api/v1/requests",
        Some(json!({"media_type":"movie","tmdb_id":603})),
    )
    .await;
    assert_eq!(s, 201, "{r}");
    let rid = r["id"].as_i64().unwrap();

    let mut seen: Vec<(String, u64)> = vec![];
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(end, ws.next()).await {
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] != "request_progress" || v["request_id"] != rid {
            continue;
        }
        assert_eq!(v["tmdb_id"], 603);
        let state = v["state"].as_str().unwrap().to_string();
        seen.push((
            state.clone(),
            (v["progress"].as_f64().unwrap() * 100.0).round() as u64,
        ));
        if state == "importing" {
            break;
        }
    }
    let states: Vec<&str> = seen.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        states,
        [
            "requested",
            "approved",
            "searching",
            "queued",
            "downloading",
            "importing"
        ],
        "{seen:?}"
    );
    assert_eq!(seen[4].1, 50);
    assert_eq!(seen[5].1, 100);

    let (_, active) = get(&e.app, "/api/v1/requests/active").await;
    assert_eq!(active[0]["state"], "importing");
    assert_eq!(active[0]["release_title"], "Matrix.1999.1080p");
}

fn make_video(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x48:rate=5",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
        ])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[tokio::test]
async fn webhook_import_ingests_file_and_marks_available() {
    let have = |t: &str| Command::new(t).arg("-version").output().is_ok();
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return;
    }
    let tmdb = MockServer::start().await;
    mock_tmdb_movie(&tmdb, 603, "The Matrix").await;
    let radarr = MockServer::start().await;
    mock_radarr(&radarr, 603, 42).await;
    let e = env(&tmdb, false).await;
    add_radarr(&e.app, &radarr.uri()).await;

    // Empty movies library; the file arrives later via the webhook.
    let movies = e.dir.path().join("movies");
    std::fs::create_dir_all(&movies).unwrap();
    let (s, lib) = send(
        &e.app,
        "POST",
        "/api/v1/libraries",
        Some(json!({"name":"Movies","kind":"movies","paths":[movies]})),
    )
    .await;
    assert_eq!(s, 201);
    let lib_id = lib["id"].as_i64().unwrap();

    let (_, r) = send(
        &e.app,
        "POST",
        "/api/v1/requests",
        Some(json!({"media_type":"movie","tmdb_id":603})),
    )
    .await;
    let rid = r["id"].as_i64().unwrap();
    assert_eq!(r["status"], "searching");

    let file = movies.join("The Matrix (1999)/The Matrix (1999).mp4");
    make_video(&file);
    let payload = json!({
        "eventType": "Download",
        "movie": {"id": 42, "tmdbId": 603, "title": "The Matrix"},
        "movieFile": {"path": file}
    });
    let url = |tok: &str| format!("/api/v1/webhooks/arr?server=1&token={tok}");
    assert_eq!(
        send(&e.app, "POST", &url("wrong"), Some(payload.clone()))
            .await
            .0,
        403
    );
    assert_eq!(
        send(
            &e.app,
            "POST",
            "/api/v1/webhooks/arr",
            Some(payload.clone())
        )
        .await
        .0,
        403
    );
    assert_eq!(
        send(&e.app, "POST", &url("s3cret"), Some(json!({"nope":1})))
            .await
            .0,
        400
    );

    let (s, v) = send(&e.app, "POST", &url("s3cret"), Some(payload)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["event"], "Download");
    assert_eq!(v["request_ids"], json!([rid]));
    assert_eq!(v["scanned_paths"].as_array().unwrap().len(), 1);

    // ingested …
    let (_, items) = get(&e.app, &format!("/api/v1/libraries/{lib_id}/items")).await;
    assert_eq!(items["items"][0]["title"], "The Matrix");
    // … and the request is available
    let (_, l) = get(&e.app, "/api/v1/requests?status=available").await;
    assert_eq!(l[0]["id"], rid);
    assert_eq!(get(&e.app, "/api/v1/requests/active").await.1, json!([]));

    // Test event is accepted without side effects.
    let (s, v) = send(
        &e.app,
        "POST",
        &url("s3cret"),
        Some(json!({"eventType":"Test"})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["request_ids"], json!([]));
}

#[tokio::test]
async fn webhooks_disabled_without_secret() {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let app = app(AppState::new(db, cfg));
    let (s, _) = send(
        &app,
        "POST",
        "/api/v1/webhooks/arr?token=",
        Some(json!({"eventType":"Test"})),
    )
    .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn notifications_and_download_clients_admin() {
    let tmdb = MockServer::start().await;
    let hook = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&hook)
        .await;
    let e = env(&tmdb, false).await;

    let (s, n) = send(
        &e.app,
        "POST",
        "/api/v1/admin/notifications",
        Some(json!({"kind":"discord","config":{"webhook_url": format!("{}/hook", hook.uri())},"events":["available"]})),
    )
    .await;
    assert_eq!(s, 201, "{n}");
    assert!(
        n["config"]["webhook_url"]
            .as_str()
            .unwrap()
            .starts_with("••••")
    );
    assert!(!n.to_string().contains(&hook.uri()));
    assert_eq!(n["events"], json!(["available"]));
    let id = n["id"].as_i64().unwrap();
    assert_eq!(
        get(&e.app, "/api/v1/admin/notifications").await.1[0]["id"],
        id
    );
    assert_eq!(
        send(
            &e.app,
            "POST",
            &format!("/api/v1/admin/notifications/{id}/test"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(&e.app, "POST", "/api/v1/admin/notifications/99/test", None)
            .await
            .0,
        404
    );
    // validation
    for bad in [
        json!({"kind":"discord","config":{}}),
        json!({"kind":"sms","config":{}}),
        json!({"kind":"webhook","config":{"url":"http://x"},"events":["nope"]}),
    ] {
        assert_eq!(
            send(&e.app, "POST", "/api/v1/admin/notifications", Some(bad))
                .await
                .0,
            400
        );
    }
    assert_eq!(
        send(
            &e.app,
            "DELETE",
            &format!("/api/v1/admin/notifications/{id}"),
            None
        )
        .await
        .0,
        204
    );

    // download clients: password never returned
    let (s, c) = send(
        &e.app,
        "POST",
        "/api/v1/admin/download-clients",
        Some(json!({"kind":"qbittorrent","url":"http://qbit:8080","username":"admin","password":"hunter2"})),
    )
    .await;
    assert_eq!(s, 201);
    assert_eq!(c["password_set"], true);
    assert!(!c.to_string().contains("hunter2"));
    let cid = c["id"].as_i64().unwrap();
    assert_eq!(
        get(&e.app, "/api/v1/admin/download-clients")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        send(
            &e.app,
            "POST",
            "/api/v1/admin/download-clients",
            Some(json!({"kind":"deluge","url":"http://x"}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        send(
            &e.app,
            "DELETE",
            &format!("/api/v1/admin/download-clients/{cid}"),
            None
        )
        .await
        .0,
        204
    );
}

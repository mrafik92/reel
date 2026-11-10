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

struct Fx {
    app: Router,
    _dir: tempfile::TempDir,
    movies: i64,
    shows: i64,
}

async fn call(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn json_of(res: Response<Body>) -> Value {
    let b = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&b).unwrap()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let res = call(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    let s = res.status();
    (s, json_of(res).await)
}

async fn post_library(app: &Router, name: &str, kind: &str, path: &Path) -> Response<Body> {
    let body = json!({"name": name, "kind": kind, "paths": [path]});
    call(
        app,
        Request::post("/api/v1/libraries")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn wait_items(app: &Router, lib: i64, n: usize) {
    for _ in 0..200 {
        let (_, v) = get(app, &format!("/api/v1/libraries/{lib}/items?limit=200")).await;
        if v["items"].as_array().map(Vec::len) == Some(n) {
            // let a scan finish ingesting children too
            tokio::time::sleep(Duration::from_millis(300)).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scan did not produce {n} items");
}

async fn fixture() -> Option<Fx> {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe missing; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let mv = dir.path().join("movies");
    for (t, y) in [("Alpha", 2001), ("Beta Max", 2002), ("Gamma", 2003)] {
        make_video(&mv.join(format!("{t} ({y})/{t} ({y}).mp4")));
    }
    let tv = dir.path().join("tv");
    for e in 1..=2 {
        make_video(&tv.join(format!(
            "Cool Show (2010)/Season 01/Cool Show - S01E0{e}.mp4"
        )));
    }
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let state = AppState::new(db, cfg);
    let app = common::authed(&state, app(state.clone())).await;

    let res = post_library(&app, "Movies", "movies", &mv).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let movies = json_of(res).await["id"].as_i64().unwrap();
    let res = post_library(&app, "TV", "shows", &tv).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let shows = json_of(res).await["id"].as_i64().unwrap();
    wait_items(&app, movies, 3).await;
    wait_items(&app, shows, 1).await;
    Some(Fx {
        app,
        _dir: dir,
        movies,
        shows,
    })
}

#[tokio::test]
async fn libraries_create_and_list() {
    let Some(fx) = fixture().await else { return };
    let (s, v) = get(&fx.app, "/api/v1/libraries").await;
    assert_eq!(s, 200);
    let libs = v.as_array().unwrap();
    assert_eq!(libs.len(), 2);
    assert_eq!(libs[0]["name"], "Movies");
    assert_eq!(libs[0]["kind"], "movies");
    assert_eq!(libs[0]["item_count"], 3);
    assert!(libs[0]["paths"][0].as_str().unwrap().contains("movies"));
    assert_eq!(libs[1]["item_count"], 1);

    let res = post_library(&fx.app, "Bad", "movies", Path::new("/definitely/not/here")).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = call(
        &fx.app,
        Request::post(format!("/api/v1/libraries/{}/scan", fx.movies))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let res = call(
        &fx.app,
        Request::post("/api/v1/libraries/999/scan")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn items_pagination_all_sorts() {
    let Some(fx) = fixture().await else { return };
    for sort in ["title", "added", "year"] {
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut uri = format!("/api/v1/libraries/{}/items?sort={sort}&limit=2", fx.movies);
            if let Some(c) = &cursor {
                uri.push_str(&format!("&cursor={c}"));
            }
            let (s, v) = get(&fx.app, &uri).await;
            assert_eq!(s, 200);
            for it in v["items"].as_array().unwrap() {
                assert_eq!(it["kind"], "movie");
                seen.push(it["title"].as_str().unwrap().to_string());
            }
            match v["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
        }
        assert_eq!(seen.len(), 3, "sort={sort}: {seen:?}");
        let mut u = seen.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), 3);
        if sort == "title" {
            assert_eq!(seen, ["Alpha", "Beta Max", "Gamma"]);
        }
        if sort == "year" {
            assert_eq!(seen, ["Gamma", "Beta Max", "Alpha"]);
        }
    }
    let (s, _) = get(
        &fx.app,
        &format!("/api/v1/libraries/{}/items?cursor=garbage", fx.movies),
    )
    .await;
    assert_eq!(s, 400);
    // shows library lists only the show
    let (_, v) = get(&fx.app, &format!("/api/v1/libraries/{}/items", fx.shows)).await;
    assert_eq!(v["items"][0]["kind"], "show");
}

#[tokio::test]
async fn item_detail_children_and_search() {
    let Some(fx) = fixture().await else { return };
    let (_, v) = get(
        &fx.app,
        &format!("/api/v1/libraries/{}/items?sort=year", fx.movies),
    )
    .await;
    let movie_id = v["items"][0]["id"].as_i64().unwrap();
    assert!(v["items"][0].get("poster_url").is_none());

    let (s, d) = get(&fx.app, &format!("/api/v1/items/{movie_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(d["kind"], "movie");
    assert_eq!(d["played"], false);
    assert_eq!(d["children_count"], 0);
    assert_eq!(d["genres"], json!([]));
    assert_eq!(d["cast"], json!([]));
    let f = &d["files"][0];
    assert_eq!(f["container"], "mp4");
    assert!(
        f["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("/api/v1/direct/{}?exp=", f["id"]))
    );
    assert_eq!(f["video"]["codec"], "mpeg4");
    assert_eq!(f["video"]["width"], 64);
    assert_eq!(f["video"]["hdr"], "none");
    assert_eq!(f["audio"][0]["codec"], "aac");
    assert!(f["audio"][0]["channels"].as_i64().unwrap() >= 1);
    assert_eq!(f["subtitles"], json!([]));
    assert!(f["duration_s"].as_f64().unwrap() > 0.5);

    let (_, v) = get(&fx.app, &format!("/api/v1/libraries/{}/items", fx.shows)).await;
    let show = v["items"][0]["id"].as_i64().unwrap();
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{show}")).await;
    assert_eq!(d["children_count"], 1);
    assert_eq!(d["files"], json!([]));
    let (_, seasons) = get(&fx.app, &format!("/api/v1/items/{show}/children")).await;
    assert_eq!(seasons[0]["kind"], "season");
    assert_eq!(seasons[0]["season_no"], 1);
    assert_eq!(seasons[0]["parent_id"], show);
    let sid = seasons[0]["id"].as_i64().unwrap();
    let (_, eps) = get(&fx.app, &format!("/api/v1/items/{sid}/children")).await;
    let nos: Vec<i64> = eps
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["episode_no"].as_i64().unwrap())
        .collect();
    assert_eq!(nos, [1, 2]);
    assert_eq!(eps[0]["kind"], "episode");

    assert_eq!(get(&fx.app, "/api/v1/items/99999").await.0, 404);
    assert_eq!(get(&fx.app, "/api/v1/items/99999/children").await.0, 404);

    // search: prefix ranks before substring; case-insensitive; LIKE chars escaped
    let (_, r) = get(&fx.app, "/api/v1/search?q=ALPH").await;
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!(r[0]["title"], "Alpha");
    let (_, r) = get(&fx.app, "/api/v1/search?q=a").await;
    assert_eq!(r[0]["title"], "Alpha"); // prefix match first
    assert!(r.as_array().unwrap().len() >= 3);
    let (_, r) = get(&fx.app, "/api/v1/search?q=%25").await;
    assert_eq!(r, json!([]));
    let (_, r) = get(&fx.app, "/api/v1/search?q=cool").await;
    assert_eq!(r[0]["kind"], "show");
}

#[tokio::test]
async fn home_continue_watching() {
    let Some(fx) = fixture().await else { return };
    let (_, h) = get(&fx.app, "/api/v1/home").await;
    assert_eq!(h["continue_watching"], json!([]));
    assert_eq!(h["recently_added"].as_array().unwrap().len(), 4);
    assert_eq!(h["rows"].as_array().unwrap().len(), 2);
    assert_eq!(h["rows"][0]["title"], "Movies");
}

#[tokio::test]
async fn direct_range_and_images() {
    let Some(fx) = fixture().await else { return };
    let (_, v) = get(
        &fx.app,
        &format!("/api/v1/libraries/{}/items?sort=title", fx.movies),
    )
    .await;
    let id = v["items"][0]["id"].as_i64().unwrap();
    let (_, d) = get(&fx.app, &format!("/api/v1/items/{id}")).await;
    let fid = d["files"][0]["id"].as_i64().unwrap();
    let size = d["files"][0]["size"].as_i64().unwrap();

    let res = call(
        &fx.app,
        Request::get(format!("/api/v1/direct/{fid}"))
            .header(header::RANGE, "bytes=0-99")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        res.headers()[header::CONTENT_RANGE],
        format!("bytes 0-99/{size}")
    );
    assert_eq!(res.headers()[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(
        res.into_body().collect().await.unwrap().to_bytes().len(),
        100
    );

    let res = call(
        &fx.app,
        Request::get(format!("/api/v1/direct/{fid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(get(&fx.app, "/api/v1/direct/99999").await.0, 404);

    // images: absent -> 404
    for u in [
        format!("/api/v1/images/poster/{id}?w=342"),
        format!("/api/v1/images/backdrop/{id}"),
        "/api/v1/images/person/1".to_string(),
        "/api/v1/images/poster/99999".to_string(),
        "/api/v1/images/bogus/1".to_string(),
    ] {
        assert_eq!(get(&fx.app, &u).await.0, 404, "{u}");
    }
}

#[tokio::test]
async fn image_served_with_cache_headers() {
    if !have("ffmpeg") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("poster.png");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=400x600",
                "-frames:v",
                "1"
            ])
            .arg(&img)
            .status()
            .unwrap()
            .success()
    );
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO libraries (id, name, kind, paths_json) VALUES (1, 'M', 'movies', ?)")
        .bind(serde_json::json!([dir.path().to_str().unwrap()]).to_string())
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, poster) VALUES (1, 1, 'movie', 'X', ?)")
        .bind(format!("file://{}", img.display()))
        .execute(&db)
        .await
        .unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let pool = db.clone();
    let state = AppState::new(db, cfg);
    let app = common::authed(&state, app(state.clone())).await;
    let (_, d) = get(&app, "/api/v1/items/1").await;
    assert!(
        d["poster_url"]
            .as_str()
            .unwrap()
            .starts_with("/api/v1/images/poster/1?w=342&exp="),
        "{d}"
    );

    let res = call(
        &app,
        Request::get("/api/v1/images/poster/1?w=185")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        res.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000"
    );
    let etag = res.headers()[header::ETAG].clone();
    let res = call(
        &app,
        Request::get("/api/v1/images/poster/1?w=185")
            .header(header::IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    // progress + continue watching
    sqlx::query("UPDATE media_items SET runtime_s = 100 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO watch_state (user_id, item_id, position_s) VALUES (1, 1, 25)")
        .execute(&pool)
        .await
        .unwrap();
    let (_, h) = get(&app, "/api/v1/home").await;
    assert_eq!(h["continue_watching"][0]["id"], 1);
    assert_eq!(h["continue_watching"][0]["progress"], 0.25);
    sqlx::query("UPDATE watch_state SET played = 1")
        .execute(&pool)
        .await
        .unwrap();
    let (_, h) = get(&app, "/api/v1/home").await;
    assert_eq!(h["continue_watching"], json!([]));
}

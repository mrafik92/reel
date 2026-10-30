mod common;

use std::{
    path::{Path as FsPath, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get as route_get,
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{Limits, NewUser, Role};
use reel_metadata::TmdbClient;
use serde_json::{Value, json};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

// 1x1 PNG
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F,
    0x00, 0x05, 0xFE, 0x02, 0xFE, 0xA7, 0x35, 0x81, 0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-encoders")
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("libx264") && s.contains(" aac ") && s.contains("mpeg4")
        })
        .unwrap_or(false)
}

fn encode(path: &FsPath, vcodec: &[&str], secs: u32) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=160x120:rate=25:duration={secs}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=duration={secs}"))
        .args(vcodec)
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-g", "25"])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed");
}

#[derive(Default)]
struct Xt {
    /// every `action` the panel was asked for
    actions: Mutex<Vec<String>>,
    /// Range headers of media requests
    ranges: Mutex<Vec<String>>,
    block_info: AtomicU16,
    info_calls: AtomicUsize,
    media_hits: AtomicUsize,
    live_active: AtomicUsize,
    mp4: PathBuf,
    mkv: PathBuf,
    ts: Vec<u8>,
}

fn img(p: &str) -> String {
    format!("__BASE__/img/{p}.png")
}

fn movies() -> Value {
    let mut v = vec![
        json!({"num":1,"name":"AR| Dune (2021)","stream_id":1001,"stream_icon":img("p1"),"rating":"8.0","added":"1700000300","category_id":"10","container_extension":"mp4"}),
        json!({"num":2,"name":"Amélie","stream_id":1002,"stream_icon":img("p2"),"rating":"8.3","added":"1700000200","category_id":"10","container_extension":"mkv"}),
    ];
    for (i, (r, a)) in [(6.0, 100), (7.0, 110), (5.5, 120), (9.0, 130)]
        .into_iter()
        .enumerate()
    {
        v.push(json!({"num":3+i,"name":format!("Filler Film {}", i + 3),"stream_id":1003+i,"stream_icon":img("p"),
            "rating":r.to_string(),"added":(1_700_000_000i64 + a).to_string(),"category_id":"10","container_extension":"mp4"}));
    }
    v.push(json!({"num":7,"name":"Other Movie (2010)","stream_id":1007,"stream_icon":"","rating":"0","added":"1700000050","category_id":"11","container_extension":"mp4","tmdb":"555"}));
    v.push(json!({"num":8,"name":"Lonely","stream_id":1008,"stream_icon":"","rating":"4","added":"1700000010","category_id":"11","container_extension":"mp4"}));
    Value::Array(v)
}

async fn panel_api(
    State((x, base)): State<(Arc<Xt>, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let action = q.get("action").cloned().unwrap_or_default();
    x.actions.lock().unwrap().push(action.clone());
    let body = match action.as_str() {
        "get_live_categories" => json!([{"category_id":"1","category_name":"General"}]),
        "get_live_streams" => {
            json!([{"num":1,"name":"News One","stream_id":101,"stream_icon":"","epg_channel_id":"","category_id":"1"}])
        }
        "get_vod_categories" => {
            json!([{"category_id":"10","category_name":"Drama"},{"category_id":"11","category_name":"Misc"}])
        }
        "get_vod_streams" => movies(),
        "get_series_categories" => json!([{"category_id":"20","category_name":"Series A"}]),
        "get_series" => json!([
            {"series_id":2001,"name":"UK| Show One (2019)","cover":img("s1"),"rating":"7.5","last_modified":"1700000500","category_id":"20","plot":"List plot","genre":"Drama / Crime"},
            {"series_id":2002,"name":"Show Two","cover":"","rating":"6","last_modified":"1700000400","category_id":"20"}
        ]),
        "get_vod_info" => {
            x.info_calls.fetch_add(1, SeqCst);
            let code = x.block_info.load(SeqCst);
            if code != 0 {
                return StatusCode::from_u16(code).unwrap().into_response();
            }
            json!({"info":{"tmdb_id":"438631","plot":"Provider plot","youtube_trailer":"https://www.youtube.com/watch?v=abcDEF12345",
                "director":"Denis V","genre":"Sci-Fi / Adventure","duration_secs":9300,"backdrop_path":[img("b1")],
                "cast":"Timothée Chalamet, Zendaya","releasedate":"2021-09-15","rating":"8.0","cover_big":img("p1")},
                "movie_data":{"stream_id":1001,"container_extension":"mp4"}})
        }
        "get_series_info" => json!({
        "seasons":[{"season_number":1,"name":"Season 1","cover":img("s1")}],
        "info":{"name":"Show One","plot":"Series plot","cast":"A. Actor","director":"D","genre":"Drama / Crime","tmdb":"7777","backdrop_path":[img("b2")]},
        "episodes":{
          "1":[{"id":"5001","episode_num":1,"title":"Pilot","container_extension":"mp4","season":1,"info":{"plot":"ep plot","duration_secs":4,"movie_image":img("e1")}},
               {"id":"5002","episode_num":2,"title":"Second","container_extension":"mp4","season":1,"info":{"duration_secs":4}}],
          "2":[{"id":"5003","episode_num":1,"title":"Return","container_extension":"mp4","season":2,"info":{}}]
        }}),
        "get_short_epg" => json!({"epg_listings":[]}),
        _ => {
            json!({"user_info":{"auth":1,"status":"Active","max_connections":"1","active_cons":"0"},"server_info":{"url":base}})
        }
    };
    // artwork urls point back at this panel
    let text = body.to_string().replace("__BASE__", &base);
    ([(header::CONTENT_TYPE, "application/json")], text).into_response()
}

async fn serve_movie(
    State((x, _)): State<(Arc<Xt>, String)>,
    Path((_u, _p, file)): Path<(String, String, String)>,
    req: Request<Body>,
) -> Response {
    x.media_hits.fetch_add(1, SeqCst);
    if let Some(r) = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        x.ranges.lock().unwrap().push(r.to_string());
    }
    let id: i64 = file.split('.').next().unwrap().parse().unwrap();
    let p = if id == 1002 {
        x.mkv.clone()
    } else {
        x.mp4.clone()
    };
    ServeFile::new(p)
        .oneshot(req)
        .await
        .unwrap()
        .into_response()
}

struct Guard(Arc<Xt>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.live_active.fetch_sub(1, SeqCst);
    }
}

async fn serve_live(State((x, _)): State<(Arc<Xt>, String)>) -> Response {
    x.live_active.fetch_add(1, SeqCst);
    let g = Guard(x.clone());
    let data = bytes::Bytes::from(x.ts.clone());
    let s = futures_util::stream::unfold(
        (g, 0usize, tokio::time::interval(Duration::from_millis(20))),
        move |(g, pos, mut t)| {
            let data = data.clone();
            async move {
                t.tick().await;
                let n = 1880usize.min(data.len() - pos);
                Some((
                    Ok::<_, std::io::Error>(data.slice(pos..pos + n)),
                    (g, (pos + n) % data.len(), t),
                ))
            }
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp2t")
        .body(Body::from_stream(s))
        .unwrap()
}

async fn serve_img() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], PNG).into_response()
}

struct Env {
    x: Arc<Xt>,
    base: String,
    state: AppState,
    admin: Router,
    raw: Router,
    source_id: i64,
    tmdb: MockServer,
    _dir: tempfile::TempDir,
}

async fn start_panel(dir: &FsPath) -> (Arc<Xt>, String) {
    let mp4 = dir.join("a.mp4");
    let mkv = dir.join("b.mkv");
    let ts = dir.join("c.ts");
    encode(
        &mp4,
        &[
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-movflags",
            "+faststart",
        ],
        4,
    );
    encode(&mkv, &["-c:v", "mpeg4"], 4);
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&mp4)
        .args(["-c", "copy", "-f", "mpegts"])
        .arg(&ts)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let x = Arc::new(Xt {
        mp4,
        mkv,
        ts: std::fs::read(ts).unwrap(),
        ..Default::default()
    });
    let st = (x.clone(), base.clone());
    let app = Router::new()
        .route("/player_api.php", route_get(panel_api))
        .route("/movie/{u}/{p}/{file}", route_get(serve_movie))
        .route("/series/{u}/{p}/{file}", route_get(serve_movie))
        .route("/live/{u}/{p}/{file}", route_get(serve_live))
        .route("/img/{name}", route_get(serve_img))
        .with_state(st);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (x, base)
}

async fn env(cfg: impl FnOnce(&mut reel_core::Config), max_conn: i64) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let (x, base) = start_panel(dir.path()).await;
    let tmdb = MockServer::start().await;
    Mock::given(method("GET")).and(path("/movie/438631"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 438631, "title": "Dune", "overview": "TMDB plot", "backdrop_path": "/bd.jpg", "runtime": 155,
            "genres": [{"id":1,"name":"Sci-Fi"}],
            "credits": {"cast": [
                {"id": 1190668, "name": "Timothée Chalamet", "character": "Paul", "profile_path": "/tc.jpg", "order": 0},
                {"id": 505710, "name": "Zendaya", "character": "Chani", "profile_path": null, "order": 1}], "crew": []}
        })))
        .mount(&tmdb).await;
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let mut c = reel_core::Config {
        data_dir: dir.path().join("data"),
        allow_private_logo_fetch: true,
        ..Default::default()
    };
    cfg(&mut c);
    let state = AppState::new(db.clone(), c).with_tmdb(TmdbClient::with_base_url("k", tmdb.uri()));
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    let source_id: i64 = sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password, max_connections) \
         VALUES ('Panel','xtream',?, 'bob', 'hunter2', ?) RETURNING id",
    )
    .bind(&base)
    .bind(max_conn)
    .fetch_one(&db)
    .await
    .unwrap();
    state.iptv.refresh_source(source_id).await.unwrap();
    Env {
        x,
        base,
        state,
        admin,
        raw,
        source_id,
        tmdb,
        _dir: dir,
    }
}

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

async fn user_app(e: &Env, name: &str) -> (Router, i64) {
    let u = e
        .state
        .auth
        .create_user(NewUser {
            username: name.into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids: vec![],
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login(name, "another long password", Default::default())
        .await
        .unwrap()
        .token;
    (common::with_token(e.raw.clone(), &t), u.id)
}

fn titles(v: &Value) -> Vec<&str> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect()
}

async fn raw_get(app: &Router, uri: &str, range: Option<&str>) -> Response {
    let mut r = Request::builder().uri(uri);
    if let Some(x) = range {
        r = r.header(header::RANGE, x);
    }
    app.clone()
        .oneshot(r.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn catalog_ingest_lists_filters_sorts_pages_and_keeps_ids_stable() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    // counts on the admin source DTO
    let (_, src) = get(
        &e.admin,
        &format!("/api/v1/admin/iptv/sources/{}", e.source_id),
    )
    .await;
    assert_eq!(
        (
            src["vod_movie_count"].as_i64(),
            src["vod_series_count"].as_i64()
        ),
        (Some(8), Some(2))
    );

    let (s, cats) = get(&e.admin, "/api/v1/vod/categories?type=movie").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        cats,
        json!([{"id": cats[0]["id"], "name":"Drama","count":6},{"id": cats[1]["id"], "name":"Misc","count":2}])
    );
    let drama = cats[0]["id"].as_i64().unwrap();
    let (_, scats) = get(&e.admin, "/api/v1/vod/categories?type=series").await;
    assert_eq!(scats[0]["count"], 2);
    assert_eq!(
        get(&e.admin, "/api/v1/vod/categories?type=nope").await.0,
        StatusCode::BAD_REQUEST
    );

    let (s, p) = get(&e.admin, "/api/v1/vod/movies?sort=added").await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(titles(&p)[..3], ["Dune", "Amélie", "Filler Film 6"], "{p}");
    let dune = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Dune")
        .unwrap()
        .clone();
    assert_eq!(
        (
            dune["kind"].as_str(),
            dune["year"].as_i64(),
            dune["rating"].as_f64()
        ),
        (Some("movie"), Some(2021), Some(8.0))
    );
    assert_eq!(dune["category_id"], drama);
    assert!(
        dune["poster_url"]
            .as_str()
            .unwrap()
            .contains("/api/v1/vod/images/movie_poster/")
    );
    let other = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Other Movie")
        .unwrap()
        .clone();
    assert_eq!(
        (other["year"].as_i64(), other["tmdb_id"].as_i64()),
        (Some(2010), Some(555)),
        "year from the name, tmdb from the list"
    );
    assert!(other.get("poster_url").is_none() && other.get("rating").is_none());

    // filters
    assert_eq!(
        titles(
            &get(
                &e.admin,
                &format!("/api/v1/vod/movies?category={drama}&sort=rating&limit=3")
            )
            .await
            .1
        ),
        ["Filler Film 6", "Amélie", "Dune"]
    );
    assert_eq!(
        titles(&get(&e.admin, "/api/v1/vod/movies?q=AMELIE").await.1),
        ["Amélie"],
        "diacritics + case"
    );
    assert_eq!(
        titles(
            &get(&e.admin, "/api/v1/vod/movies?q=filler&sort=title")
                .await
                .1
        ),
        [
            "Filler Film 3",
            "Filler Film 4",
            "Filler Film 5",
            "Filler Film 6"
        ]
    );
    assert_eq!(
        get(&e.admin, "/api/v1/vod/movies?sort=bogus").await.0,
        StatusCode::BAD_REQUEST
    );
    // cursor pagination covers everything once
    for sort in ["added", "rating", "title", "year"] {
        let mut seen = vec![];
        let mut cursor = String::new();
        loop {
            let (s, pg) = get(
                &e.admin,
                &format!("/api/v1/vod/movies?sort={sort}&limit=3&cursor={cursor}"),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{sort}: {pg}");
            seen.extend(
                pg["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].as_i64().unwrap()),
            );
            match pg["next_cursor"].as_str() {
                Some(c) => cursor = c.to_string(),
                None => break,
            }
        }
        let mut u = seen.clone();
        u.sort();
        u.dedup();
        assert_eq!((seen.len(), u.len()), (8, 8), "{sort}");
    }
    // series
    let (_, sp) = get(&e.admin, "/api/v1/vod/series").await;
    assert_eq!(titles(&sp), ["Show One", "Show Two"]);
    assert_eq!(sp["items"][0]["kind"], "series");
    assert_eq!(sp["items"][0]["year"], 2019);
    assert_eq!(sp["items"][0]["genres"], json!(["Drama", "Crime"]));

    // refresh: ids stay stable, vanished titles are hidden, watch state survives
    let id_of = |t: &str, p: &Value| {
        p["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["title"] == t)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let dune_id = id_of("Dune", &p);
    sqlx::query("INSERT INTO vod_watch (user_id, kind, item_id, position_s, duration_s, played) VALUES (1,'movie',?,10,100,1)")
        .bind(dune_id).execute(&e.state.db).await.unwrap();
    e.state.iptv.refresh_source(e.source_id).await.unwrap();
    let (_, p2) = get(&e.admin, "/api/v1/vod/movies").await;
    assert_eq!(id_of("Dune", &p2), dune_id);
    let d = p2["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == dune_id)
        .unwrap();
    assert_eq!(
        (d["played"].as_bool(), d["progress"].as_f64()),
        (Some(true), Some(0.1))
    );
    assert_eq!(
        e.x.actions
            .lock()
            .unwrap()
            .iter()
            .filter(|a| *a == "get_vod_streams")
            .count(),
        2
    );
}

#[tokio::test]
async fn details_load_provider_info_lazily_enrich_with_tmdb_and_cache() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    let (_, p) = get(&e.admin, "/api/v1/vod/movies?q=dune").await;
    let id = p["items"][0]["id"].as_i64().unwrap();
    assert_eq!(e.x.info_calls.load(SeqCst), 0, "nothing fetched at ingest");
    let (s, d) = get(&e.admin, &format!("/api/v1/vod/movies/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["title"], "Dune");
    assert_eq!(d["overview"], "Provider plot");
    assert_eq!(d["director"], "Denis V");
    assert_eq!(d["trailer_youtube_id"], "abcDEF12345");
    assert_eq!(
        (
            d["runtime_s"].as_i64(),
            d["container"].as_str(),
            d["tmdb_id"].as_i64()
        ),
        (Some(9300), Some("mp4"), Some(438631))
    );
    assert_eq!(d["genres"], json!(["Sci-Fi", "Adventure"]));
    assert!(
        d["backdrop_url"]
            .as_str()
            .unwrap()
            .contains("/vod/images/movie_backdrop/")
    );
    assert_eq!(d["added_at"], 1_700_000_300);
    // cast from TMDB: person ids + photos
    assert_eq!(d["cast"][0]["name"], "Timothée Chalamet");
    assert_eq!(d["cast"][0]["person_tmdb_id"], 1190668);
    assert_eq!(d["cast"][0]["character"], "Paul");
    assert!(
        d["cast"][0]["photo_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/profile")
    );
    // second view: cached, the provider is not asked again
    get(&e.admin, &format!("/api/v1/vod/movies/{id}")).await;
    assert_eq!(e.x.info_calls.load(SeqCst), 1);
    assert_eq!(
        get(&e.admin, "/api/v1/vod/movies/99999").await.0,
        StatusCode::NOT_FOUND
    );

    // series: seasons + episodes from get_series_info
    let (_, sp) = get(&e.admin, "/api/v1/vod/series?q=show%20one").await;
    let sid = sp["items"][0]["id"].as_i64().unwrap();
    let (s, sd) = get(&e.admin, &format!("/api/v1/vod/series/{sid}")).await;
    assert_eq!(s, StatusCode::OK, "{sd}");
    assert_eq!(sd["overview"], "Series plot");
    let seasons = sd["seasons"].as_array().unwrap();
    assert_eq!(seasons.len(), 2);
    assert_eq!(
        (
            seasons[0]["season_no"].as_i64(),
            seasons[0]["name"].as_str()
        ),
        (Some(1), Some("Season 1"))
    );
    assert!(
        seasons[0]["poster_url"]
            .as_str()
            .unwrap()
            .contains("/vod/images/season_poster/")
    );
    let eps = seasons[0]["episodes"].as_array().unwrap();
    assert_eq!(
        eps.iter()
            .map(|x| x["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Pilot", "Second"]
    );
    assert_eq!(
        (
            eps[0]["episode_no"].as_i64(),
            eps[0]["overview"].as_str(),
            eps[0]["runtime_s"].as_i64()
        ),
        (Some(1), Some("ep plot"), Some(4))
    );
    assert!(
        eps[0]["still_url"]
            .as_str()
            .unwrap()
            .contains("/vod/images/episode_still/")
    );
    assert_eq!(seasons[1]["episodes"][0]["title"], "Return");
    assert_eq!(sd["tmdb_id"], 7777);
    assert_eq!(sd["cast"][0]["name"], "A. Actor");
    let _ = &e.tmdb;
}

#[tokio::test]
async fn provider_blocking_the_info_call_does_not_break_details_and_pauses_the_provider() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    e.x.block_info.store(469, SeqCst);
    let (_, p) = get(&e.admin, "/api/v1/vod/movies?q=dune").await;
    let id = p["items"][0]["id"].as_i64().unwrap();
    let (s, d) = get(&e.admin, &format!("/api/v1/vod/movies/{id}")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "details still render from the catalog: {d}"
    );
    assert_eq!(d["title"], "Dune");
    assert!(d.get("director").is_none());
    assert_eq!(e.x.info_calls.load(SeqCst), 1);
    // the breaker is open: another detail view does not call the provider
    let (_, p2) = get(&e.admin, "/api/v1/vod/movies?q=amelie").await;
    let id2 = p2["items"][0]["id"].as_i64().unwrap();
    assert_eq!(
        get(&e.admin, &format!("/api/v1/vod/movies/{id2}")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        e.x.info_calls.load(SeqCst),
        1,
        "no request while the provider is rate limiting us"
    );
    assert!(
        e.state
            .iptv
            .gates
            .get(e.source_id)
            .penalty_remaining()
            .is_some()
    );
}

#[tokio::test]
async fn lanes_images_search_and_discover_links() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    let (s, lanes) = get(&e.admin, "/api/v1/vod/lanes?type=movie").await;
    assert_eq!(s, StatusCode::OK, "{lanes}");
    let l = lanes.as_array().unwrap();
    assert_eq!(l[0]["kind"], "new");
    assert_eq!(l[0]["title"], "Recently added");
    assert_eq!(l[0]["items"][0]["title"], "Dune");
    assert_eq!(
        l[0]["see_all"],
        json!({"type": "movie", "query": "sort=added"})
    );
    assert_eq!(l[1]["kind"], "top_unwatched");
    assert_eq!(
        l[1]["items"][0]["title"], "Filler Film 6",
        "best rated first"
    );
    let cat = l.iter().find(|x| x["kind"] == "genre").unwrap();
    assert_eq!(cat["title"], "Drama");
    assert_eq!(cat["subtitle"], "6 titles");
    assert!(
        cat["see_all"]["query"]
            .as_str()
            .unwrap()
            .starts_with("category=")
    );
    assert!(l.iter().all(|x| x["items"].as_array().unwrap().len() <= 20));
    // a played movie drops out of "top rated"
    let top_id = l[1]["items"][0]["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO vod_watch (user_id, kind, item_id, position_s, played) VALUES (1,'movie',?,0,1)").bind(top_id).execute(&e.state.db).await.unwrap();
    let (_, l2) = get(&e.admin, "/api/v1/vod/lanes?type=movie").await;
    assert_ne!(l2[1]["items"][0]["id"], json!(top_id));
    // series lanes are well-formed (too few titles → no lanes)
    assert_eq!(
        get(&e.admin, "/api/v1/vod/lanes?type=series").await.0,
        StatusCode::OK
    );

    // artwork is proxied (private targets allowed here), signed, and image-sniffed
    let poster = get(&e.admin, "/api/v1/vod/movies?q=dune").await.1["items"][0]["poster_url"]
        .as_str()
        .unwrap()
        .to_string();
    let res = raw_get(&e.raw, &poster, None).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        raw_get(&e.raw, "/api/v1/vod/images/movie_poster/1", None)
            .await
            .status(),
        401,
        "unsigned"
    );
    assert!(!poster.contains("hunter2") && !poster.contains(&e.base));

    // search/all gets the VOD section; the discover card links to the VOD entry
    Mock::given(method("GET")).and(path("/search/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 438631, "title": "Dune", "release_date": "2021-09-15", "popularity": 90.0},
            {"id": 9999, "title": "Other Movie", "release_date": "2010-06-01", "popularity": 50.0},
            {"id": 8888, "title": "Nothing Here", "release_date": "2000-01-01", "popularity": 10.0}]})))
        .mount(&e.tmdb).await;
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&e.tmdb)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/person"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&e.tmdb)
        .await;
    let (s, v) = get(&e.admin, "/api/v1/search/all?q=dune").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["vod"][0]["title"], "Dune");
    assert_eq!(v["vod"][0]["kind"], "movie");
    let dune_vod = v["vod"][0]["id"].as_i64().unwrap();
    let d: Vec<&Value> = v["discover"].as_array().unwrap().iter().collect();
    let dune_card = d.iter().find(|c| c["tmdb_id"] == 438631).unwrap();
    assert_eq!(
        dune_card["vod_movie_id"], dune_vod,
        "matched by tmdb id (set from the provider info or list)"
    );
    let (_, v) = get(&e.admin, "/api/v1/search/all?q=movie").await;
    let other = v["discover"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["tmdb_id"] == 9999)
        .unwrap()
        .clone();
    assert!(
        other["vod_movie_id"].is_i64(),
        "matched by normalized title + year: {other}"
    );
    let nothing = v["discover"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["tmdb_id"] == 8888)
        .unwrap()
        .clone();
    assert!(nothing.get("vod_movie_id").is_none());

    // the default SSRF policy (private logo fetches off) refuses loopback artwork
    let e2 = env(|c| c.allow_private_logo_fetch = false, 1).await;
    let poster2 = get(&e2.admin, "/api/v1/vod/movies?q=dune").await.1["items"][0]["poster_url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(raw_get(&e2.raw, &poster2, None).await.status(), 404);
}

#[tokio::test]
async fn direct_play_is_proxied_with_range_and_progress_is_persisted() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    let (_, p) = get(&e.admin, "/api/v1/vod/movies?q=dune").await;
    let id = p["items"][0]["id"].as_i64().unwrap();
    let (s, d) = send(
        &e.admin,
        "POST",
        "/api/v1/vod/play",
        Some(json!({"kind":"movie","id":id,"preset":"web_chrome"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["mode"], "direct_play");
    let url = d["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/v1/vod/direct/"), "{url}");
    assert!(
        !d.to_string().contains("hunter2") && !d.to_string().contains(&e.base),
        "provider URL/credentials never reach clients: {d}"
    );
    assert!(!d["audio"].as_array().unwrap().is_empty());
    // probe cached without credentials
    let pj: String =
        sqlx::query_scalar("SELECT probe_json FROM vod_probe WHERE kind = 'movie' AND item_id = ?")
            .bind(id)
            .fetch_one(&e.state.db)
            .await
            .unwrap();
    assert!(!pj.contains("hunter2") && pj.contains("h264"), "{pj}");

    // proxied range request (no bearer: the signature is enough)
    let full = std::fs::read(&e.x.mp4).unwrap();
    let res = raw_get(&e.raw, &url, Some("bytes=100-199")).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        res.headers()[header::CONTENT_RANGE],
        format!("bytes 100-199/{}", full.len())
    );
    assert_eq!(res.headers()[header::CONTENT_TYPE], "video/mp4");
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], &full[100..200]);
    assert_eq!(
        e.x.ranges.lock().unwrap().last().map(String::as_str),
        Some("bytes=100-199"),
        "Range forwarded upstream"
    );
    let res = raw_get(&e.raw, &url, None).await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.into_body().collect().await.unwrap().to_bytes().len(),
        full.len()
    );
    assert_eq!(
        raw_get(&e.raw, "/api/v1/vod/direct/nope", None)
            .await
            .status(),
        401
    );
    let (other, _) = user_app(&e, "mallory").await;
    assert_eq!(
        raw_get(&other, url.split('?').next().unwrap(), None)
            .await
            .status(),
        404,
        "another user's bearer cannot use the session"
    );

    // progress → vod_watch → shown on cards; finishing marks played
    let sid = d["session_id"].as_str().unwrap();
    let (s, _) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{sid}/progress"),
        Some(json!({"position_s": 1.0})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, card) = get(&e.admin, "/api/v1/vod/movies?q=dune").await;
    let c = &card["items"][0];
    assert_eq!(c["played"], false);
    assert!(
        c["progress"].as_f64().unwrap() > 0.1 && c["progress"].as_f64().unwrap() < 0.5,
        "{c}"
    );
    send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{sid}/progress"),
        Some(json!({"position_s": 3.9})),
    )
    .await;
    assert_eq!(
        get(&e.admin, "/api/v1/vod/movies?q=dune").await.1["items"][0]["played"],
        true
    );

    // stopping releases the provider slot
    assert_eq!(e.state.iptv.live.vod_lease_count(), 1);
    assert_eq!(
        send(&e.admin, "DELETE", &format!("/api/v1/playback/{sid}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(e.state.iptv.live.vod_lease_count(), 0);
    assert!(e.state.vod.get(sid).is_none());
    // episodes play too
    let (_, sp) = get(&e.admin, "/api/v1/vod/series?q=show%20one").await;
    let (_, sd) = get(
        &e.admin,
        &format!("/api/v1/vod/series/{}", sp["items"][0]["id"]),
    )
    .await;
    let ep = sd["seasons"][0]["episodes"][0]["id"].as_i64().unwrap();
    let (s, d) = send(
        &e.admin,
        "POST",
        "/api/v1/vod/play",
        Some(json!({"kind":"episode","id":ep})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["mode"], "direct_play");
    send(
        &e.admin,
        "POST",
        &format!(
            "/api/v1/playback/{}/progress",
            d["session_id"].as_str().unwrap()
        ),
        Some(json!({"position_s": 4.0})),
    )
    .await;
    let (_, sd) = get(
        &e.admin,
        &format!("/api/v1/vod/series/{}", sp["items"][0]["id"]),
    )
    .await;
    assert_eq!(sd["seasons"][0]["episodes"][0]["played"], true);
    assert_eq!(sd["seasons"][0]["episodes"][1]["played"], false);
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/vod/play",
            Some(json!({"kind":"movie","id":424242}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/vod/play",
            Some(json!({"kind":"x","id":1}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn unsupported_codecs_are_transcoded_from_the_remote_input() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|_| {}, 1).await;
    let (_, p) = get(&e.admin, "/api/v1/vod/movies?q=amelie").await;
    let id = p["items"][0]["id"].as_i64().unwrap();
    // MPEG-4 part 2 in MKV: the browser cannot play it → ffmpeg reads the provider URL
    let (s, d) = send(
        &e.admin,
        "POST",
        "/api/v1/vod/play",
        Some(json!({"kind":"movie","id":id,"preset":"web_chrome"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_ne!(d["mode"], "direct_play", "{d}");
    assert!(
        d["url"].as_str().unwrap().contains("/stream/")
            && d["url"].as_str().unwrap().contains("master.m3u8")
    );
    assert!(!d.to_string().contains("hunter2"));
    let res = raw_get(&e.raw, d["url"].as_str().unwrap(), None).await;
    assert_eq!(res.status(), 200);
    let text =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.contains("#EXT-X-STREAM-INF"), "{text}");
    // ffmpeg really pulled the movie over HTTP (the probe made the first request)
    let mut tries = 0;
    while e.x.media_hits.load(SeqCst) < 2 && tries < 100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        tries += 1;
    }
    assert!(
        e.x.media_hits.load(SeqCst) >= 2,
        "ffmpeg never connected to the provider"
    );
    let sid = d["session_id"].as_str().unwrap();
    send(&e.admin, "DELETE", &format!("/api/v1/playback/{sid}"), None).await;
}

#[tokio::test]
async fn vod_shares_the_provider_connection_budget_with_live_tv() {
    if !have_ffmpeg() {
        return;
    }
    let e = env(|c| c.iptv_grace_secs = 30, 1).await;
    let (_, p) = get(&e.admin, "/api/v1/vod/movies?q=dune").await;
    let movie = p["items"][0]["id"].as_i64().unwrap();
    let chan: i64 = sqlx::query_scalar("SELECT id FROM channels WHERE ext_id = '101'")
        .fetch_one(&e.state.db)
        .await
        .unwrap();
    let (alice, alice_id) = user_app(&e, "alice").await;
    let (bob, _) = user_app(&e, "bob").await;
    let play = |app: &Router, body: Value| {
        let app = app.clone();
        async move { send(&app, "POST", "/api/v1/vod/play", Some(body)).await }
    };
    let tune = |app: &Router, body: Value| {
        let app = app.clone();
        async move {
            send(
                &app,
                "POST",
                &format!("/api/v1/iptv/channels/{chan}/tune"),
                Some(body),
            )
            .await
        }
    };
    let vp = || json!({"kind":"movie","id":movie});

    // alice watches live TV; her VOD play takes over her own slot (newest wins)
    let (s, live) = tune(&alice, json!({"mode":"passthrough"})).await;
    assert_eq!(s, StatusCode::OK, "{live}");
    let (s, v1) = play(&alice, vp()).await;
    assert_eq!(s, StatusCode::OK, "own streams never 429: {v1}");
    assert!(
        e.state
            .iptv
            .live
            .session(live["session_id"].as_str().unwrap())
            .is_none(),
        "live session stopped"
    );
    assert_eq!(e.state.iptv.live.vod_lease_count(), 1);

    // bob: friendly 429 for non-admins, 409 + takeover for admins
    let (s, v) = play(&bob, vp()).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("in use by other viewers")
    );
    let (s, v) = tune(&bob, json!({"mode":"passthrough"})).await;
    assert_eq!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "live tune sees the VOD holder too: {v}"
    );
    let (s, v) = play(&e.admin, vp()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(
        (
            v["takeover"].as_bool(),
            v["holders"][0]["username"].as_str()
        ),
        (Some(true), Some("alice"))
    );
    assert_eq!(v["holders"][0]["channel_name"], "Dune");
    let (s, v) = play(&e.admin, json!({"kind":"movie","id":movie,"takeover":true})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // alice's VOD session was stopped by the takeover
    let alice_sid = v1["session_id"].as_str().unwrap();
    for _ in 0..40 {
        if e.state.playback.session(alice_sid).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(e.state.playback.session(alice_sid).await.is_err());
    assert!(e.state.vod.get(alice_sid).is_none());
    let _ = alice_id;

    // the admin tunes live TV: their own VOD playback gives way (newest wins) …
    let (s, t) = tune(&e.admin, json!({"mode":"passthrough"})).await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(e.state.iptv.live.vod_lease_count(), 0);
    // … and the live holder blocks bob again until it is stopped
    assert_eq!(play(&bob, vp()).await.0, StatusCode::TOO_MANY_REQUESTS);
    send(
        &e.admin,
        "DELETE",
        &format!(
            "/api/v1/iptv/sessions/{}",
            t["session_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let (s, b1) = play(&bob, vp()).await;
    assert_eq!(s, StatusCode::OK, "{b1}");

    // an idle-reaped playback session releases its slot too
    let bsid = b1["session_id"].as_str().unwrap().to_string();
    assert_eq!(e.state.iptv.live.vod_lease_count(), 1);
    e.state.playback.remove(&bsid).await;
    assert_eq!(e.state.vod_reap().await, 1);
    assert_eq!(e.state.iptv.live.vod_lease_count(), 0);
    let (s, _) = tune(&alice, json!({"mode":"passthrough"})).await;
    assert_eq!(s, StatusCode::OK);
    e.state.iptv.shutdown().await;
}

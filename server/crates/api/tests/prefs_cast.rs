mod common;

use std::{path::Path, process::Command};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{Limits, NewUser, Role};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

async fn send(app: &Router, m: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, _, v) = send_h(app, m, uri, body, &[]).await;
    (s, v)
}

async fn send_h(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut r = Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let s = res.status();
    let h = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, h, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct Env {
    state: AppState,
    admin: Router,
    raw: Router,
    db: SqlitePool,
    dir: tempfile::TempDir,
}

async fn env(cast_base: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        cast_base_url: cast_base.map(str::to_string),
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    Env {
        state,
        admin,
        raw,
        db,
        dir,
    }
}

async fn user(e: &Env, name: &str) -> Router {
    let have_lib: i64 = sqlx::query_scalar("SELECT count(*) FROM libraries WHERE id = 1")
        .fetch_one(&e.db)
        .await
        .unwrap();
    let library_ids = if have_lib > 0 { vec![1] } else { vec![] };
    e.state
        .auth
        .create_user(NewUser {
            username: name.into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids,
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
    common::with_token(e.raw.clone(), &t)
}

type StreamRow<'a> = (
    i64,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    i64,
    i64,
    Option<&'a str>,
);

/// A movie file (mp4/h264) with Arabic default audio, English audio and a set of subtitle tracks.
async fn library_file(e: &Env, real_file: Option<&Path>) -> i64 {
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(&e.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, kind, title) VALUES (1,1,'movie','Polyglot')",
    )
    .execute(&e.db)
    .await
    .unwrap();
    let path = real_file.map_or("/nowhere/polyglot.mp4".to_string(), |p| {
        p.to_string_lossy().into_owned()
    });
    let fid: i64 = sqlx::query_scalar("INSERT INTO media_files (item_id, path, size, mtime, container, duration_s) VALUES (1, ?, 1, 1, 'mp4', 3600) RETURNING id")
        .bind(path).fetch_one(&e.db).await.unwrap();
    // idx, kind, codec, lang, title, default, forced, external
    let rows: [StreamRow; 7] = [
        (0, "video", "h264", None, None, 1, 0, None),
        (1, "audio", "aac", Some("ara"), None, 1, 0, None),
        (2, "audio", "aac", Some("eng"), None, 0, 0, None),
        (
            3,
            "subtitle",
            "subrip",
            Some("eng"),
            Some("English"),
            0,
            0,
            None,
        ),
        (
            4,
            "subtitle",
            "subrip",
            Some("eng"),
            Some("English forced"),
            0,
            1,
            None,
        ),
        (
            5,
            "subtitle",
            "subrip",
            Some("ara"),
            Some("Arabic forced"),
            0,
            1,
            None,
        ),
        (
            6,
            "subtitle",
            "srt",
            Some("en"),
            Some("Polyglot.en.sdh.srt"),
            0,
            0,
            Some("/m/Polyglot.en.sdh.srt"),
        ),
    ];
    for (idx, kind, codec, lang, title, def, forced, ext) in rows {
        let (w, h, ch) = if kind == "video" {
            (Some(128), Some(96), None)
        } else if kind == "audio" {
            (None, None, Some(2))
        } else {
            (None, None, None)
        };
        sqlx::query("INSERT INTO streams (file_id, idx, kind, codec, language, title, \"default\", forced, external_path, width, height, channels) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(fid).bind(idx).bind(kind).bind(codec).bind(lang).bind(title).bind(def).bind(forced).bind(ext).bind(w).bind(h).bind(ch)
            .execute(&e.db).await.unwrap();
    }
    fid
}

#[tokio::test]
async fn preferences_api_defaults_normalization_validation_and_isolation() {
    let e = env(None).await;
    let (s, d) = send(&e.admin, "GET", "/api/v1/me/preferences", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        d,
        json!({"audio_languages":["original"],"subtitle_mode":"auto","subtitle_languages":["en"],"prefer_sdh":false,"burn_in_image_subs":false})
    );
    // codes are normalised (639-2, names, regions), duplicates dropped, order kept
    let (s, d) = send(&e.admin, "PUT", "/api/v1/me/preferences", Some(json!({
        "audio_languages": ["Arabic", "ara", "original", "nld"], "subtitle_mode": "forced_only",
        "subtitle_languages": ["eng", "Dutch", "en-GB"], "prefer_sdh": true, "burn_in_image_subs": true}))).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["audio_languages"], json!(["ar", "original", "nl"]));
    assert_eq!(d["subtitle_languages"], json!(["en", "nl"]));
    assert_eq!(
        send(&e.admin, "GET", "/api/v1/me/preferences", None)
            .await
            .1,
        d,
        "persisted"
    );
    // partial PUT: the rest falls back to the defaults
    let (_, d) = send(
        &e.admin,
        "PUT",
        "/api/v1/me/preferences",
        Some(json!({"subtitle_mode": "always"})),
    )
    .await;
    assert_eq!(
        (
            d["subtitle_mode"].as_str(),
            d["audio_languages"].clone(),
            d["prefer_sdh"].as_bool()
        ),
        (Some("always"), json!(["original"]), Some(false))
    );
    // validation
    for bad in [
        json!({"subtitle_mode": "sometimes"}),
        json!({"audio_languages": ["klingon-ish-123"]}),
        json!({"subtitle_languages": ["original"]}),
        json!({"audio_languages": ["en","fr","de","es","it","nl","pt","ru","ja","ko","zh"]}),
    ] {
        let (s, v) = send(&e.admin, "PUT", "/api/v1/me/preferences", Some(bad.clone())).await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {s} {v}"
        );
    }
    assert_eq!(
        send(
            &e.admin,
            "PUT",
            "/api/v1/me/preferences",
            Some(json!({"audio_languages": []}))
        )
        .await
        .1["audio_languages"],
        json!(["original"])
    );
    // per user + auth
    let other = user(&e, "dana").await;
    assert_eq!(
        send(&other, "GET", "/api/v1/me/preferences", None).await.1["subtitle_mode"],
        "auto"
    );
    assert_eq!(
        send(&e.raw, "GET", "/api/v1/me/preferences", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn decide_selects_audio_and_subtitles_from_the_preferences() {
    let e = env(None).await;
    let fid = library_file(&e, None).await;
    let decide = |body: Value| {
        let app = e.admin.clone();
        async move {
            send(
                &app,
                "POST",
                &format!("/api/v1/playback/{fid}/decide"),
                Some(body),
            )
            .await
        }
    };
    // defaults: original → the file's default audio (Arabic); Arabic is not one of the user's
    // languages → the full English track (the embedded one, not SDH)
    let (s, d) = decide(json!({"preset": "webos"})).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["selected"]["audio_index"], 1);
    assert_eq!(d["selected"]["subtitle_index"], 3);
    assert!(
        d["selected"]["reason"].as_str().unwrap().contains("Arabic"),
        "{d}"
    );
    assert_eq!(d["decision"]["audio_index"], 1);
    assert_eq!(d["decision"]["subtitle_index"], 3);
    // sidecar WebVTT URLs for text tracks
    assert!(
        d["subtitles"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["url"].is_string())
            .count()
            == 4
    );

    let put = |p: Value| {
        let app = e.admin.clone();
        async move { send(&app, "PUT", "/api/v1/me/preferences", Some(p)).await }
    };
    // English audio preferred → understood → only the forced English track
    put(json!({"audio_languages": ["en"]})).await;
    let (_, d) = decide(json!({"preset": "webos"})).await;
    assert_eq!(
        (
            d["selected"]["audio_index"].as_i64(),
            d["selected"]["subtitle_index"].as_i64()
        ),
        (Some(2), Some(4)),
        "{d}"
    );
    // forced_only follows the audio language
    put(json!({"audio_languages": ["ar"], "subtitle_mode": "forced_only"})).await;
    assert_eq!(
        decide(json!({"preset": "webos"})).await.1["selected"]["subtitle_index"],
        5
    );
    // always: full English; with prefer_sdh the SDH sidecar (tokens `.en.sdh.`)
    put(json!({"audio_languages": ["ar"], "subtitle_mode": "always"})).await;
    assert_eq!(
        decide(json!({"preset": "webos"})).await.1["selected"]["subtitle_index"],
        3
    );
    put(json!({"audio_languages": ["ar"], "subtitle_mode": "always", "prefer_sdh": true})).await;
    assert_eq!(
        decide(json!({"preset": "webos"})).await.1["selected"]["subtitle_index"],
        6
    );
    // off
    put(json!({"subtitle_mode": "off"})).await;
    assert!(decide(json!({"preset": "webos"})).await.1["selected"]["subtitle_index"].is_null());
    // the client's indexes win; -1 = no subtitles
    put(json!({"subtitle_mode": "always"})).await;
    let (_, d) = decide(json!({"preset": "webos", "audio_index": 2, "subtitle_index": 5})).await;
    assert_eq!(
        (
            d["selected"]["audio_index"].as_i64(),
            d["selected"]["subtitle_index"].as_i64()
        ),
        (Some(2), Some(5))
    );
    assert!(d["selected"]["reason"].as_str().unwrap().contains("client"));
    assert!(
        decide(json!({"preset": "webos", "subtitle_index": -1}))
            .await
            .1["selected"]["subtitle_index"]
            .is_null()
    );
    // preferences are per user
    let dana = user(&e, "dana").await;
    let (_, d) = send(
        &dana,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(json!({"preset": "webos"})),
    )
    .await;
    assert_eq!(
        d["selected"]["subtitle_index"], 3,
        "dana still has the defaults"
    );
}

fn make_mp4(path: &Path) {
    let ok = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=128x96:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "ffmpeg missing");
}

fn exp_of(url: &str) -> i64 {
    url.split(['?', '&'])
        .find_map(|p| p.strip_prefix("exp="))
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn cast_urls_are_absolute_long_lived_and_use_the_chromecast_profile() {
    let e = env(Some("http://10.10.10.207:8420/")).await;
    let fid = library_file(&e, None).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // for_cast: preset defaults to chromecast, URLs are absolute with an 8 h lifetime
    let (s, d) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(json!({"for_cast": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let url = d["url"].as_str().unwrap();
    assert!(
        url.starts_with("http://10.10.10.207:8420/api/v1/direct/"),
        "{url}"
    );
    assert!(
        url.contains(&format!("?s={}", d["session_id"].as_str().unwrap())),
        "session id lets receiver requests keep it alive: {url}"
    );
    assert!(exp_of(url) >= now + 8 * 3600 - 120, "8 h lifetime");
    let sub = d["subtitles"][0]["url"].as_str().unwrap();
    assert!(
        sub.starts_with("http://10.10.10.207:8420/api/v1/subs/")
            && exp_of(sub) >= now + 8 * 3600 - 120,
        "{sub}"
    );
    // the decision suits the receiver: Chromecast cannot render the Arabic track → fine, but
    // MP4/H.264/AAC plays; presets list contains the new ones
    assert_eq!(d["mode"], "direct_play");
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/playback/{fid}/decide"),
            Some(json!({"preset":"airplay"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/playback/{fid}/decide"),
            Some(json!({"preset":"nope"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // later: absolute URLs for a running session (e.g. when the user clicks Cast mid-playback)
    let sid = d["session_id"].as_str().unwrap();
    let (s, c) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{sid}/cast-url"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["session_id"], sid);
    assert_eq!(c["base_url"], "http://10.10.10.207:8420");
    assert!(
        c["url"]
            .as_str()
            .unwrap()
            .starts_with("http://10.10.10.207:8420/api/v1/direct/")
    );
    assert_eq!(c["content_type"], "video/mp4");
    assert_eq!(c["expires_in_s"], 8 * 3600);
    let subs = c["subtitles"].as_array().unwrap();
    assert_eq!(subs.len(), 4);
    assert!(subs.iter().all(|s| {
        s["url"]
            .as_str()
            .unwrap()
            .starts_with("http://10.10.10.207:8420/api/v1/subs/")
    }));
    // only the owner (or an admin) may ask
    let dana = user(&e, "dana").await;
    assert_eq!(
        send(
            &dana,
            "POST",
            &format!("/api/v1/playback/{sid}/cast-url"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &e.raw,
            "POST",
            &format!("/api/v1/playback/{sid}/cast-url"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // without a configured base URL the server cannot make cast URLs
    let e2 = env(None).await;
    let fid2 = library_file(&e2, None).await;
    let (s, d) = send(
        &e2.admin,
        "POST",
        &format!("/api/v1/playback/{fid2}/decide"),
        Some(json!({"for_cast": true})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(d["error"].as_str().unwrap().contains("cast_base_url"));
    let (_, plain) = send(
        &e2.admin,
        "POST",
        &format!("/api/v1/playback/{fid2}/decide"),
        Some(json!({"preset":"web_chrome"})),
    )
    .await;
    assert!(
        plain["url"]
            .as_str()
            .unwrap()
            .starts_with("/api/v1/direct/"),
        "normal URLs stay relative"
    );
    let _ = &e.dir;
}

/// #309: the TV is next to the sender. A sender on the home LAN gets URLs on the LAN base; one
/// on Tailscale (routed through the gateway) or the internet gets them on the public name.
#[tokio::test]
async fn cast_urls_follow_the_senders_network() {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        cast_base_url: Some("http://10.10.10.207:8420".into()),
        public_url: Some("https://reel.example.com/".into()),
        trusted_proxies: vec!["10.10.10.73".into()],
        cast_lan_networks: vec!["10.10.10.0/24".into(), "!10.10.10.1".into()],
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    let e = Env {
        state,
        admin,
        raw,
        db,
        dir,
    };
    let fid = library_file(&e, None).await;

    // (socket peer, X-Forwarded-For) → expected base
    let cases = [
        ("10.10.10.42", None, "http://10.10.10.207:8420/"),
        (
            "10.10.10.73",
            Some("10.10.10.42"),
            "http://10.10.10.207:8420/",
        ),
        (
            "10.10.10.73",
            Some("10.10.10.1"),
            "https://reel.example.com/",
        ),
        (
            "10.10.10.73",
            Some("85.144.1.2"),
            "https://reel.example.com/",
        ),
        ("100.101.102.103", None, "https://reel.example.com/"),
        // an untrusted peer can't claim to be on the LAN
        (
            "203.0.113.9",
            Some("10.10.10.42"),
            "https://reel.example.com/",
        ),
    ];
    for (peer, xff, want) in cases {
        let call = |uri: String, body: Option<Value>| {
            let mut r = Request::builder().method("POST").uri(uri);
            if let Some(x) = xff {
                r = r.header("x-forwarded-for", x);
            }
            let b = match body {
                Some(v) => {
                    r = r.header(header::CONTENT_TYPE, "application/json");
                    Body::from(v.to_string())
                }
                None => Body::empty(),
            };
            let mut req = r.body(b).unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                    peer.parse().unwrap(),
                    50000,
                )));
            let app = e.admin.clone();
            async move {
                let res = app.oneshot(req).await.unwrap();
                let s = res.status();
                let bytes = res.into_body().collect().await.unwrap().to_bytes();
                (s, serde_json::from_slice::<Value>(&bytes).unwrap())
            }
        };
        let (s, d) = call(
            format!("/api/v1/playback/{fid}/decide"),
            Some(json!({"for_cast": true})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{peer} {xff:?}: {d}");
        let url = d["url"].as_str().unwrap();
        assert!(url.starts_with(want), "{peer} {xff:?}: {url}");
        assert!(
            d["subtitles"][0]["url"].as_str().unwrap().starts_with(want),
            "{peer} {xff:?}"
        );
        let sid = d["session_id"].as_str().unwrap().to_string();
        let (s, c) = call(format!("/api/v1/playback/{sid}/cast-url"), None).await;
        assert_eq!(s, StatusCode::OK, "{c}");
        assert_eq!(
            c["base_url"].as_str().unwrap(),
            want.trim_end_matches('/'),
            "{peer} {xff:?}"
        );
        // normal playback stays relative wherever the sender is
        let (_, plain) = call(
            format!("/api/v1/playback/{fid}/decide"),
            Some(json!({"preset": "web_chrome"})),
        )
        .await;
        assert!(plain["url"].as_str().unwrap().starts_with("/api/v1/"));
    }
}

#[tokio::test]
async fn signed_media_routes_answer_cors_and_expose_range_headers() {
    let e = env(Some("http://10.10.10.207:8420")).await;
    let mp4 = e.dir.path().join("movie.mp4");
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    make_mp4(&mp4);
    let fid = library_file(&e, Some(&mp4)).await;
    let (_, d) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(json!({"preset":"web_chrome"})),
    )
    .await;
    let url = d["url"].as_str().unwrap().to_string();

    // preflight from a receiver origin
    let (s, h, _) = send_h(
        &e.raw,
        "OPTIONS",
        &url,
        None,
        &[
            ("origin", "https://www.gstatic.com"),
            ("access-control-request-method", "GET"),
            ("access-control-request-headers", "range"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(h["access-control-allow-origin"], "*");
    assert!(
        h["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("GET")
    );
    assert!(
        h["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .to_lowercase()
            .contains("range")
    );
    // the media response itself carries CORS + exposes the range headers
    let (s, h, _) = send_h(
        &e.raw,
        "GET",
        &url,
        None,
        &[
            ("origin", "https://www.gstatic.com"),
            ("range", "bytes=0-99"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(h["access-control-allow-origin"], "*");
    let ex = h["access-control-expose-headers"]
        .to_str()
        .unwrap()
        .to_lowercase();
    assert!(
        ex.contains("content-range") && ex.contains("accept-ranges"),
        "{ex}"
    );
    // every kind of signed media route
    for p in [
        "/api/v1/stream/x/master.m3u8",
        "/api/v1/subs/1.vtt",
        "/api/v1/vod/direct/x",
        "/api/v1/vod/subs/x/1.vtt",
        "/api/v1/iptv/live/x/stream.ts",
        "/api/v1/images/poster/1",
        "/api/v1/vod/images/movie_poster/1",
    ] {
        let (_, h, _) = send_h(
            &e.raw,
            "OPTIONS",
            p,
            None,
            &[("origin", "https://x.example")],
        )
        .await;
        assert_eq!(h["access-control-allow-origin"], "*", "{p}");
        let (st, h, _) = send_h(&e.raw, "GET", p, None, &[("origin", "https://x.example")]).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{p}: still needs a signature");
        assert_eq!(
            h["access-control-allow-origin"], "*",
            "{p}: errors are readable cross-origin too"
        );
    }
    // everything else stays same-origin only
    let (_, h, _) = send_h(
        &e.raw,
        "OPTIONS",
        "/api/v1/libraries",
        None,
        &[("origin", "https://x.example")],
    )
    .await;
    assert!(h.get("access-control-allow-origin").is_none());
    let (_, h, _) = send_h(
        &e.admin,
        "GET",
        "/api/v1/me/preferences",
        None,
        &[("origin", "https://x.example")],
    )
    .await;
    assert!(h.get("access-control-allow-origin").is_none());
}

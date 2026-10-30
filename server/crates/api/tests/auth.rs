mod common;

use std::{path::Path, process::Command, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{AuthOptions, LimiterConfig};
use reel_core::{RequestState, ServerEvent};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const PW: &str = "correct horse battery";

struct Env {
    state: AppState,
    /// No credentials attached.
    raw: Router,
    admin: Router,
    pool: SqlitePool,
    dir: tempfile::TempDir,
}

async fn env_with(cfg: impl FnOnce(&mut reel_core::Config), opts: Option<AuthOptions>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let mut c = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    cfg(&mut c);
    let mut state = AppState::new(pool.clone(), c);
    if let Some(o) = opts {
        state = state.with_auth_options(o);
    }
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    Env {
        state,
        raw,
        admin,
        pool,
        dir,
    }
}

async fn env() -> Env {
    env_with(|_| {}, None).await
}

async fn raw_call(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn body_of(res: Response<Body>) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

fn send(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    let fut = send_h(app, m, uri, body, &[]);
    async move {
        let (s, _, v) = fut.await;
        (s, v)
    }
}

fn send_h(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> impl std::future::Future<Output = (StatusCode, axum::http::HeaderMap, Value)> + use<> {
    let app = app.clone();
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
    let req = r.body(b).unwrap();
    async move {
        let res = raw_call(&app, req).await;
        let (s, h) = (res.status(), res.headers().clone());
        let bytes = body_of(res).await;
        (s, h, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

fn get(app: &Router, uri: &str) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    send(app, "GET", uri, None)
}

/// A user's router: `app` + that user's token.
fn as_token(e: &Env, token: &str) -> Router {
    common::with_token(e.raw.clone(), token)
}

/// Seed a library with one movie whose (fake) file lives on disk and is direct-playable by Chrome.
/// Returns `(library_id, item_id, file_id)`.
async fn seed_library(e: &Env, name: &str) -> (i64, i64, i64) {
    let file = e.dir.path().join(format!("{name}.mp4"));
    std::fs::write(&file, vec![7u8; 4096]).unwrap();
    let lib: i64 = sqlx::query_scalar(
        "INSERT INTO libraries (name, kind, paths_json) VALUES (?, 'movies', '[]') RETURNING id",
    )
    .bind(name)
    .fetch_one(&e.pool)
    .await
    .unwrap();
    let item: i64 = sqlx::query_scalar(
        "INSERT INTO media_items (library_id, kind, title, sort_title, year) \
         VALUES (?, 'movie', ?, lower(?), 2020) RETURNING id",
    )
    .bind(lib)
    .bind(format!("{name} Movie"))
    .bind(format!("{name} Movie"))
    .fetch_one(&e.pool)
    .await
    .unwrap();
    let file_id: i64 = sqlx::query_scalar(
        "INSERT INTO media_files (item_id, path, size, mtime, container, duration_s, bitrate) \
         VALUES (?, ?, 4096, 1, 'mp4', 600, 4000000) RETURNING id",
    )
    .bind(item)
    .bind(file.to_str().unwrap())
    .fetch_one(&e.pool)
    .await
    .unwrap();
    for (idx, kind, codec, w, h, ch) in [
        (0, "video", "h264", 1280, 720, 0),
        (1, "audio", "aac", 0, 0, 2),
    ] {
        sqlx::query(
            "INSERT INTO streams (file_id, idx, kind, codec, width, height, bit_depth, channels, profile, level) \
             VALUES (?, ?, ?, ?, ?, ?, 8, ?, 'high', 40)",
        )
        .bind(file_id)
        .bind(idx)
        .bind(kind)
        .bind(codec)
        .bind(w)
        .bind(h)
        .bind(ch)
        .execute(&e.pool)
        .await
        .unwrap();
    }
    (lib, item, file_id)
}

/// Admin creates an invite and `username` redeems it; returns the new user's token + id.
async fn invited_user(e: &Env, username: &str, libs: &[i64], limits: Value) -> (String, i64) {
    let (s, inv) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/invites",
        Some(json!({"library_ids": libs, "limits": limits})),
    )
    .await;
    assert_eq!(s, 201, "{inv}");
    let code = inv["code"].as_str().unwrap();
    let (s, v) = send(
        &e.raw,
        "POST",
        &format!("/api/v1/auth/invite/{code}/accept"),
        Some(json!({"username": username, "password": PW})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    (
        v["token"].as_str().unwrap().to_string(),
        v["user"]["id"].as_i64().unwrap(),
    )
}

#[tokio::test]
async fn setup_flow_locks_after_first_admin() {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let raw = app(AppState::new(pool, cfg));

    assert_eq!(
        get(&raw, "/api/v1/setup").await.1,
        json!({"needs_setup": true})
    );
    // Everything else needs a token; health stays public.
    let (s, h, _) = send_h(&raw, "GET", "/api/v1/libraries", None, &[]).await;
    assert_eq!(s, 401);
    assert_eq!(h[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(get(&raw, "/api/v1/health").await.0, 200);
    // The seeded placeholder account cannot be logged into.
    let login = |u: &str, p: &str| {
        send(
            &raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": u, "password": p})),
        )
    };
    assert_eq!(login("admin", "!").await.0, 401);

    let bad = |b: Value| send(&raw, "POST", "/api/v1/setup", Some(b));
    assert_eq!(bad(json!({"username":"ab","password":PW})).await.0, 400);
    assert_eq!(
        bad(json!({"username":"admin","password":"short"})).await.0,
        400
    );
    let (s, v) =
        bad(json!({"username":"Admin","password":PW,"device_id":"d1","client":"web"})).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["user"]["role"], "admin");
    assert_eq!(v["user"]["username"], "Admin");
    assert!(
        v["user"].get("library_ids").is_none(),
        "admins see all libraries"
    );
    let token = v["token"].as_str().unwrap();
    assert!(token.len() >= 43);

    // Locked from now on.
    assert_eq!(
        get(&raw, "/api/v1/setup").await.1,
        json!({"needs_setup": false})
    );
    assert_eq!(bad(json!({"username":"evil","password":PW})).await.0, 409);
    let me = get(&common::with_token(raw.clone(), token), "/api/v1/me").await;
    assert_eq!(me.0, 200);
    assert_eq!(me.1["username"], "Admin");
}

#[tokio::test]
async fn login_logout_expiry_and_sessions() {
    let e = env().await;
    let login = |dev: &str| {
        send(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(
                json!({"username":"admin","password":PW,"device_id":dev,"device_name":"Living room TV","client":"webos"}),
            ),
        )
    };
    let (s, v) = login("tv-1").await;
    assert_eq!(s, 200, "{v}");
    let t1 = v["token"].as_str().unwrap().to_string();
    assert!(v["expires_at"].as_i64().unwrap() > 0);
    // tokens are only stored hashed
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions_auth")
        .fetch_all(&e.pool)
        .await
        .unwrap();
    assert!(stored.iter().all(|h| h != &t1 && h.len() == 64));

    let tv = as_token(&e, &t1);
    assert_eq!(get(&tv, "/api/v1/me").await.0, 200);
    // wrong password / unknown user: same error
    let (s, bad) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"admin","password":"nope"})),
    )
    .await;
    let (s2, bad2) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"ghost","password":"nope"})),
    )
    .await;
    assert_eq!(
        (s, s2),
        (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED)
    );
    assert_eq!(bad, bad2);

    // one session per device: logging in again replaces the old token
    let (_, v) = login("tv-1").await;
    let t2 = v["token"].as_str().unwrap().to_string();
    assert_eq!(get(&tv, "/api/v1/me").await.0, 401);
    let tv2 = as_token(&e, &t2);
    assert_eq!(get(&tv2, "/api/v1/me").await.0, 200);

    // session list + revoking another device
    let (_, v) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"admin","password":PW,"device_id":"phone","client":"web"})),
    )
    .await;
    let t3 = v["token"].as_str().unwrap().to_string();
    let (_, list) = get(&tv2, "/api/v1/me/sessions").await;
    let tv_sess = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["device_id"] == "tv-1")
        .unwrap();
    assert_eq!(tv_sess["current"], true);
    assert_eq!(tv_sess["device_name"], "Living room TV");
    assert_eq!(tv_sess["client"], "webos");
    let phone_id = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["device_id"] == "phone")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let t3r = as_token(&e, &t3);
    assert_eq!(
        send(
            &tv2,
            "DELETE",
            &format!("/api/v1/me/sessions/{phone_id}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(get(&t3r, "/api/v1/me").await.0, 401);
    assert_eq!(
        send(
            &tv2,
            "DELETE",
            &format!("/api/v1/me/sessions/{phone_id}"),
            None
        )
        .await
        .0,
        404
    );

    // sliding expiry: a session close to expiry is extended on use
    sqlx::query(
        "UPDATE sessions_auth SET expires_at = unixepoch() + 100, last_seen = unixepoch() - 3600",
    )
    .execute(&e.pool)
    .await
    .unwrap();
    assert_eq!(get(&tv2, "/api/v1/me").await.0, 200);
    let exp: i64 =
        sqlx::query_scalar("SELECT expires_at FROM sessions_auth WHERE device_id = 'tv-1'")
            .fetch_one(&e.pool)
            .await
            .unwrap();
    assert!(exp > reel_auth::signing::now_unix() + 29 * 86_400, "{exp}");

    // expiry
    sqlx::query("UPDATE sessions_auth SET expires_at = unixepoch() - 1")
        .execute(&e.pool)
        .await
        .unwrap();
    assert_eq!(get(&tv2, "/api/v1/me").await.0, 401);

    // logout revokes
    let (_, v) = login("tv-1").await;
    let t4 = v["token"].as_str().unwrap().to_string();
    let tv4 = as_token(&e, &t4);
    assert_eq!(send(&tv4, "POST", "/api/v1/auth/logout", None).await.0, 204);
    assert_eq!(get(&tv4, "/api/v1/me").await.0, 401);
    // garbage tokens
    assert_eq!(get(&as_token(&e, "x"), "/api/v1/me").await.0, 401);
}

#[tokio::test]
async fn rate_limit_lockout_per_user_and_per_ip() {
    let opts = AuthOptions {
        limiter: LimiterConfig {
            user_max_failures: 3,
            ip_max_failures: 4,
            base_lock: Duration::from_millis(500),
            max_lock: Duration::from_secs(2),
            decay: Duration::from_secs(60),
        },
        ..Default::default()
    };
    let e = env_with(|c| c.trust_proxy_headers = true, Some(opts)).await;
    let login = |u: &str, p: &str, ip: &str| {
        send_h(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":u,"password":p})),
            &[("x-forwarded-for", ip)],
        )
    };
    // per-user: 3 failures lock the username (even for the right password)
    for _ in 0..3 {
        assert_eq!(login("admin", "wrong-password", "1.1.1.1").await.0, 401);
    }
    let (s, h, v) = login("admin", PW, "1.1.1.1").await;
    assert_eq!(s, 429, "{v}");
    let retry: u64 = h[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=2).contains(&retry));
    // another IP is also refused for that username
    assert_eq!(login("admin", PW, "2.2.2.2").await.0, 429);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(login("admin", PW, "1.1.1.1").await.0, 200);

    // per-IP: password spraying across usernames
    for i in 0..4 {
        assert_eq!(login(&format!("user{i}"), "x", "9.9.9.9").await.0, 401);
    }
    assert_eq!(login("admin", PW, "9.9.9.9").await.0, 429, "ip locked");
    assert_eq!(
        login("admin", PW, "8.8.8.8").await.0,
        200,
        "other ip unaffected"
    );

    // failures and lockouts are audited
    let (_, audit) = get(&e.admin, "/api/v1/admin/audit?limit=200").await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert!(actions.iter().filter(|a| **a == "login_failure").count() >= 7);
    assert!(actions.contains(&"login_locked"));
    assert!(actions.contains(&"setup"));
    let failure = audit
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action"] == "login_failure")
        .unwrap();
    assert!(failure["ip"].is_string());
}

#[tokio::test]
async fn invite_restricts_libraries_and_blocks_admin() {
    let e = env().await;
    let (lib_a, item_a, file_a) = seed_library(&e, "Alpha").await;
    let (lib_b, item_b, file_b) = seed_library(&e, "Beta").await;
    let (token, uid) = invited_user(&e, "bob", &[lib_a], json!({})).await;
    let bob = as_token(&e, &token);

    // invite landing page info (public) + single use
    let (_, inv) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/invites",
        Some(json!({"library_ids":[lib_b]})),
    )
    .await;
    let code = inv["code"].as_str().unwrap().to_string();
    assert!(
        inv["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/invite/{code}"))
    );
    let (_, info) = get(&e.raw, &format!("/api/v1/auth/invite/{code}")).await;
    assert_eq!(info["valid"], true);
    assert_eq!(info["library_names"], json!(["Beta"]));
    let accept = |code: &str, user: &str| {
        send(
            &e.raw,
            "POST",
            &format!("/api/v1/auth/invite/{code}/accept"),
            Some(json!({"username":user,"password":PW})),
        )
    };
    assert_eq!(accept(&code, "bob").await.0, 409, "username taken");
    assert_eq!(accept(&code, "carol").await.0, 201);
    assert_eq!(accept(&code, "dave").await.0, 404, "single use");
    assert_eq!(
        get(&e.raw, &format!("/api/v1/auth/invite/{code}")).await.1["valid"],
        false
    );
    assert_eq!(accept("nonexistent-code", "erin").await.0, 404);
    // expired
    let (_, inv2) = send(&e.admin, "POST", "/api/v1/admin/invites", Some(json!({}))).await;
    let code2 = inv2["code"].as_str().unwrap().to_string();
    sqlx::query("UPDATE invites SET expires_at = unixepoch() - 5 WHERE code = ?")
        .bind(&code2)
        .execute(&e.pool)
        .await
        .unwrap();
    assert_eq!(accept(&code2, "frank").await.0, 404);
    let (_, invs) = get(&e.admin, "/api/v1/admin/invites").await;
    let st: Vec<&str> = invs
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["status"].as_str().unwrap())
        .collect();
    assert!(st.contains(&"expired") && st.contains(&"used"));

    // bob sees only library A
    let (_, me) = get(&bob, "/api/v1/me").await;
    assert_eq!(me["role"], "user");
    assert_eq!(me["library_ids"], json!([lib_a]));
    let (_, libs) = get(&bob, "/api/v1/libraries").await;
    assert_eq!(libs.as_array().unwrap().len(), 1);
    assert_eq!(libs[0]["id"], lib_a);
    assert_eq!(
        get(&bob, &format!("/api/v1/libraries/{lib_a}/items"))
            .await
            .0,
        200
    );
    assert_eq!(
        get(&bob, &format!("/api/v1/libraries/{lib_b}/items"))
            .await
            .0,
        404
    );
    assert_eq!(get(&bob, &format!("/api/v1/items/{item_a}")).await.0, 200);
    assert_eq!(get(&bob, &format!("/api/v1/items/{item_b}")).await.0, 404);
    assert_eq!(
        get(&bob, &format!("/api/v1/items/{item_b}/children"))
            .await
            .0,
        404
    );
    let (_, found) = get(&bob, "/api/v1/search?q=movie").await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["title"], "Alpha Movie");
    assert_eq!(
        get(&e.admin, "/api/v1/search?q=movie")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, home) = get(&bob, "/api/v1/home").await;
    assert_eq!(home["rows"].as_array().unwrap().len(), 1);
    assert_eq!(home["recently_added"].as_array().unwrap().len(), 1);

    // direct play and playback decisions are library-checked (token auth)
    let direct = |f: i64| {
        Request::get(format!("/api/v1/direct/{f}"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(raw_call(&bob, direct(file_a)).await.status(), 200);
    assert_eq!(raw_call(&bob, direct(file_b)).await.status(), 404);
    let decide = |f: i64| {
        send(
            &bob,
            "POST",
            &format!("/api/v1/playback/{f}/decide"),
            Some(json!({"preset":"web_chrome"})),
        )
    };
    assert_eq!(decide(file_b).await.0, 404);
    assert_eq!(decide(file_a).await.0, 200);
    assert_eq!(
        send(
            &bob,
            "POST",
            &format!("/api/v1/items/{item_b}/played"),
            None
        )
        .await
        .0,
        404
    );

    // admin-only surface
    for (m, u) in [
        ("GET", "/api/v1/admin/users"),
        ("GET", "/api/v1/admin/audit"),
        ("GET", "/api/v1/admin/invites"),
        ("POST", "/api/v1/admin/invites"),
        ("GET", "/api/v1/admin/sessions"),
        ("GET", "/api/v1/admin/hwaccel"),
        ("GET", "/api/v1/admin/arr-servers"),
        ("GET", "/api/v1/admin/notifications"),
        ("GET", "/api/v1/admin/download-clients"),
        ("POST", "/api/v1/libraries"),
        ("POST", "/api/v1/libraries/1/scan"),
        ("POST", "/api/v1/requests/1/approve"),
        ("POST", "/api/v1/requests/1/decline"),
    ] {
        let body = (m == "POST").then(|| json!({"name":"x","kind":"movies","paths":[]}));
        let (s, _) = send(&bob, m, u, body).await;
        assert_eq!(s, 403, "{m} {u}");
    }
    // admin can edit access afterwards
    let (s, u) = send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"library_ids":[lib_a, lib_b]})),
    )
    .await;
    assert_eq!(s, 200, "{u}");
    assert_eq!(get(&bob, &format!("/api/v1/items/{item_b}")).await.0, 200);
    let _ = (lib_b, item_b);
}

#[tokio::test]
async fn user_management_and_guards() {
    let e = env().await;
    let (lib, _, _) = seed_library(&e, "Alpha").await;
    // create directly
    let (s, u) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/users",
        Some(json!({"username":"bob","password":PW,"library_ids":[lib],"limits":{"max_streams":2,"auto_approve":true}})),
    )
    .await;
    assert_eq!(s, 201, "{u}");
    let uid = u["id"].as_i64().unwrap();
    assert_eq!(u["limits"]["max_streams"], 2);
    assert_eq!(u["limits"]["auto_approve"], true);
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/users",
            Some(json!({"username":"BOB","password":PW}))
        )
        .await
        .0,
        409
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/users",
            Some(json!({"username":"x y","password":PW}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/users",
            Some(json!({"username":"zed","password":PW,"library_ids":[999]}))
        )
        .await
        .0,
        400
    );
    let (_, list) = get(&e.admin, "/api/v1/admin/users").await;
    assert_eq!(list.as_array().unwrap().len(), 2);

    // bob logs in; disabling signs him out immediately
    let (_, v) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"bob","password":PW})),
    )
    .await;
    let bob = as_token(&e, v["token"].as_str().unwrap());
    assert_eq!(get(&bob, "/api/v1/me").await.0, 200);
    let (s, u) = send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"disabled":true})),
    )
    .await;
    assert_eq!((s, u["disabled"].clone()), (StatusCode::OK, json!(true)));
    assert_eq!(get(&bob, "/api/v1/me").await.0, 401);
    assert_eq!(
        send(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"bob","password":PW}))
        )
        .await
        .0,
        401
    );
    send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"disabled":false,"password":"another long pw 1"})),
    )
    .await;
    let (s, v) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"bob","password":"another long pw 1"})),
    )
    .await;
    assert_eq!(s, 200);
    let bob = as_token(&e, v["token"].as_str().unwrap());

    // self-service password change keeps the current session, drops the others
    let (_, v2) = send(
        &e.raw,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username":"bob","password":"another long pw 1","device_id":"other"})),
    )
    .await;
    let other = as_token(&e, v2["token"].as_str().unwrap());
    let change = |cur: &str, new: &str| {
        send(
            &bob,
            "POST",
            "/api/v1/me/password",
            Some(json!({"current_password":cur,"new_password":new})),
        )
    };
    assert_eq!(change("wrong", "a brand new password").await.0, 401);
    assert_eq!(change("another long pw 1", "short").await.0, 400);
    assert_eq!(
        change("another long pw 1", "a brand new password").await.0,
        204
    );
    assert_eq!(get(&bob, "/api/v1/me").await.0, 200);
    assert_eq!(get(&other, "/api/v1/me").await.0, 401);
    assert_eq!(
        send(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"bob","password":"a brand new password"}))
        )
        .await
        .0,
        200
    );

    // revoke all sessions (admin), role change, last-admin guards
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/admin/users/{uid}/sessions"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(get(&bob, "/api/v1/me").await.0, 401);
    let (_, admin_me) = get(&e.admin, "/api/v1/me").await;
    let aid = admin_me["id"].as_i64().unwrap();
    assert_eq!(
        send(
            &e.admin,
            "PATCH",
            &format!("/api/v1/admin/users/{aid}"),
            Some(json!({"role":"user"}))
        )
        .await
        .0,
        409
    );
    assert_eq!(
        send(
            &e.admin,
            "PATCH",
            &format!("/api/v1/admin/users/{aid}"),
            Some(json!({"disabled":true}))
        )
        .await
        .0,
        409
    );
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/admin/users/{aid}"),
            None
        )
        .await
        .0,
        409
    );
    send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"role":"admin"})),
    )
    .await;
    assert_eq!(
        send(
            &e.admin,
            "PATCH",
            &format!("/api/v1/admin/users/{aid}"),
            Some(json!({"role":"user"}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        send(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"bob","password":"a brand new password"}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        send(
            &e.raw,
            "DELETE",
            &format!("/api/v1/admin/users/{uid}"),
            None
        )
        .await
        .0,
        401
    );

    // audit trail of admin actions
    let (_, audit) = get(
        &as_token(&e, &{
            let (_, v) = send(
                &e.raw,
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username":"bob","password":"a brand new password"})),
            )
            .await;
            v["token"].as_str().unwrap().to_string()
        }),
        "/api/v1/admin/audit",
    )
    .await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    for a in [
        "user_create",
        "user_update",
        "sessions_revoke",
        "password_change",
    ] {
        assert!(actions.contains(&a), "{a} in {actions:?}");
    }
}

#[tokio::test]
async fn signed_urls_valid_expired_tampered() {
    let e = env().await;
    let (_, item, file) = seed_library(&e, "Alpha").await;
    let (_, d) = get(&e.admin, &format!("/api/v1/items/{item}")).await;
    let url = d["files"][0]["url"].as_str().unwrap().to_string();
    assert!(url.contains("?exp=") && url.contains("&u=1&sig="), "{url}");

    let fetch = |u: String| async {
        raw_call(
            &e.raw,
            Request::get(u)
                .header(header::RANGE, "bytes=0-9")
                .body(Body::empty())
                .unwrap(),
        )
        .await
    };
    // valid signature, no token → 206 with the right range
    let res = fetch(url.clone()).await;
    assert_eq!(res.status(), 206);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 0-9/4096");
    // no signature, no token
    assert_eq!(fetch(format!("/api/v1/direct/{file}")).await.status(), 401);
    // tampered signature / expiry / user
    let sig = url.split("sig=").nth(1).unwrap();
    let flip = if sig.starts_with('A') { "B" } else { "A" };
    let bad_sig = format!("{flip}{}", &sig[1..]);
    let exp: i64 = url
        .split("exp=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(fetch(url.replace(sig, &bad_sig)).await.status(), 401);
    assert_eq!(
        fetch(url.replace(&format!("exp={exp}"), &format!("exp={}", exp + 1)))
            .await
            .status(),
        401
    );
    assert_eq!(fetch(url.replace("&u=1&", "&u=2&")).await.status(), 401);
    // signature of another file / scope
    let (_, _, file2) = seed_library(&e, "Beta").await;
    assert_eq!(
        fetch(url.replace(&format!("/direct/{file}"), &format!("/direct/{file2}")))
            .await
            .status(),
        401
    );
    // expired
    let now = reel_auth::signing::now_unix();
    let sig = reel_auth::signing::sign(
        e.state.auth.signing_key(),
        &format!("direct:{file}"),
        1,
        now - 10,
    );
    assert_eq!(
        fetch(format!(
            "/api/v1/direct/{file}?exp={}&u=1&sig={sig}",
            now - 10
        ))
        .await
        .status(),
        401
    );
    // a signed URL stops working once the user is disabled
    let (token, uid) = invited_user(&e, "bob", &[], json!({})).await;
    let _ = token;
    let bobsig = e.state.signer(uid).query(&format!("direct:{file}"), 600, 1);
    // bob has no library access → 404 even with a valid signature
    assert_eq!(
        fetch(format!("/api/v1/direct/{file}?{bobsig}"))
            .await
            .status(),
        404
    );
    send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"disabled":true})),
    )
    .await;
    assert_eq!(
        fetch(format!("/api/v1/direct/{file}?{bobsig}"))
            .await
            .status(),
        401
    );
    // signed image URLs work without a token, and are stable between calls
    sqlx::query("UPDATE media_items SET poster = 'file:///does/not/exist.jpg' WHERE id = ?")
        .bind(item)
        .execute(&e.pool)
        .await
        .unwrap();
    let (_, d2) = get(&e.admin, &format!("/api/v1/items/{item}")).await;
    let poster = d2["poster_url"].as_str().unwrap().to_string();
    assert_eq!(
        poster,
        get(&e.admin, &format!("/api/v1/items/{item}")).await.1["poster_url"]
    );
    // valid signature → authenticated → file missing → 404 (not 401)
    let res = raw_call(&e.raw, Request::get(&poster).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 404);
    let res = raw_call(
        &e.raw,
        Request::get(poster.replace("sig=", "sig=x"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 401);
}

fn make_video(path: &Path, acodec: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=6:size=128x96:rate=10",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=duration=6",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            acodec,
        ])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[tokio::test]
async fn stream_and_subtitle_urls_are_signed_and_owner_scoped() {
    let have = |t: &str| Command::new(t).arg("-version").output().is_ok();
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg missing; skipping");
        return;
    }
    let enc = Command::new("ffmpeg").arg("-encoders").output().unwrap();
    let enc = String::from_utf8_lossy(&enc.stdout);
    if !enc.contains("libx264") || !enc.contains(" ac3 ") {
        eprintln!("encoders missing; skipping");
        return;
    }
    let e = env().await;
    let movies = e.dir.path().join("movies");
    make_video(
        &movies.join("Needs Audio (2021)/Needs Audio (2021).mkv"),
        "ac3",
    );
    std::fs::write(
        movies.join("Needs Audio (2021)/Needs Audio (2021).en.srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nHello there\n",
    )
    .unwrap();
    let (s, lib) = send(
        &e.admin,
        "POST",
        "/api/v1/libraries",
        Some(json!({"name":"M","kind":"movies","paths":[movies]})),
    )
    .await;
    assert_eq!(s, 201);
    let lib_id = lib["id"].as_i64().unwrap();
    for _ in 0..200 {
        if get(&e.admin, "/api/v1/libraries").await.1[0]["item_count"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (_, items) = get(&e.admin, &format!("/api/v1/libraries/{lib_id}/items")).await;
    let item = items["items"][0]["id"].as_i64().unwrap();
    let (_, d) = get(&e.admin, &format!("/api/v1/items/{item}")).await;
    let fid = d["files"][0]["id"].as_i64().unwrap();
    let (s, v) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(json!({"preset":"web_chrome"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["mode"], "audio_transcode");
    let master = v["url"].as_str().unwrap().to_string();
    let sid = v["session_id"].as_str().unwrap().to_string();

    // master playlist with signature only (no token): every URI carries the signature
    let res = raw_call(&e.raw, Request::get(&master).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 200);
    let text = String::from_utf8(body_of(res).await).unwrap();
    let variant = text.lines().find(|l| l.contains("index.m3u8")).unwrap();
    assert!(
        variant.contains("?exp=") && variant.contains("&sig="),
        "{text}"
    );
    let base = master.split("master.m3u8").next().unwrap();
    let res = raw_call(
        &e.raw,
        Request::get(format!("{base}{variant}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 200);
    let media = String::from_utf8(body_of(res).await).unwrap();
    assert!(media.contains("#EXT-X-MAP:URI=\"init.mp4?exp="), "{media}");
    let seg = media.lines().find(|l| l.starts_with("seg_")).unwrap();
    assert!(seg.contains("?exp="));
    let res = raw_call(
        &e.raw,
        Request::get(format!("{base}main/{seg}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 200, "first segment");
    // unsigned / wrong-scope access
    let res = raw_call(
        &e.raw,
        Request::get(format!("/api/v1/stream/{sid}/main/index.m3u8"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 401);
    let q = master.split('?').nth(1).unwrap();
    let res = raw_call(
        &e.raw,
        Request::get(format!("/api/v1/stream/other-session/master.m3u8?{q}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 401);
    // header-authenticated owner works without a signature; other users get 404
    let res = raw_call(
        &e.admin,
        Request::get(format!("/api/v1/stream/{sid}/master.m3u8"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 200);
    let (btoken, _) = invited_user(&e, "bob", &[lib_id], json!({})).await;
    let bob = as_token(&e, &btoken);
    let res = raw_call(
        &bob,
        Request::get(format!("/api/v1/stream/{sid}/master.m3u8"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), 404);
    // bob cannot stop or report progress on admin's session
    assert_eq!(
        send(&bob, "DELETE", &format!("/api/v1/playback/{sid}"), None)
            .await
            .0,
        404
    );
    assert_eq!(
        send(
            &bob,
            "POST",
            &format!("/api/v1/playback/{sid}/progress"),
            Some(json!({"position_s":1}))
        )
        .await
        .0,
        404
    );

    // subtitle URL is signed too
    let (s, v2) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/playback/{fid}/decide"),
        Some(json!({"preset":"web_safari"})),
    )
    .await;
    assert_eq!(s, 200, "{v2}");
    let sub = v2["subtitles"][0]["url"].as_str().unwrap();
    assert!(sub.contains(".vtt?exp="));
    let res = raw_call(&e.raw, Request::get(sub).body(Body::empty()).unwrap()).await;
    assert_eq!(res.status(), 200);
    assert!(
        String::from_utf8(body_of(res).await)
            .unwrap()
            .starts_with("WEBVTT")
    );
    let bare = sub.split('?').next().unwrap();
    assert_eq!(
        raw_call(&e.raw, Request::get(bare).body(Body::empty()).unwrap())
            .await
            .status(),
        401
    );
}

#[tokio::test]
async fn max_streams_and_transcodes_limits() {
    let e = env().await;
    let (lib, _, file) = seed_library(&e, "Alpha").await;
    let (token, _) = invited_user(&e, "bob", &[lib], json!({"max_streams": 1})).await;
    let bob = as_token(&e, &token);
    let decide = || {
        send(
            &bob,
            "POST",
            &format!("/api/v1/playback/{file}/decide"),
            Some(json!({"preset":"web_chrome"})),
        )
    };
    let (s, first) = decide().await;
    assert_eq!(s, 200, "{first}");
    assert_eq!(first["mode"], "direct_play");
    let (s, v) = decide().await;
    assert_eq!(s, 429, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("limit of 1 concurrent stream")
    );
    // admin is unaffected, and the slot frees when the session stops
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/playback/{file}/decide"),
            Some(json!({"preset":"web_chrome"}))
        )
        .await
        .0,
        200
    );
    let sid = first["session_id"].as_str().unwrap();
    assert_eq!(
        send(&bob, "DELETE", &format!("/api/v1/playback/{sid}"), None)
            .await
            .0,
        204
    );
    assert_eq!(decide().await.0, 200);

    // max_bitrate_kbps comes from the user's limits: 1 Mbps cap forces a transcode
    let (_, uid) = {
        let (_, v) = get(&bob, "/api/v1/me").await;
        ((), v["id"].as_i64().unwrap())
    };
    send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/users/{uid}"),
        Some(json!({"limits":{"max_streams":5,"max_transcodes":0,"max_bitrate_kbps":1000}})),
    )
    .await;
    let (s, v) = decide().await;
    assert_eq!(s, 429, "transcode needed but max_transcodes=0: {v}");
    assert!(v["error"].as_str().unwrap().contains("transcoded"));
}

#[tokio::test]
async fn websocket_events_are_filtered_per_user() {
    let e = env().await;
    let (btoken, bob_id) = invited_user(&e, "bob", &[], json!({})).await;
    let (_, admin_me) = get(&e.admin, "/api/v1/me").await;
    let admin_id = admin_me["id"].as_i64().unwrap();
    let mk = |user: i64, tmdb: i64| {
        let pool = e.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO requests (user_id, media_type, tmdb_id, status) VALUES (?, 'movie', ?, 'searching') RETURNING id",
            )
            .bind(user)
            .bind(tmdb)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let (r_admin, r_bob) = (mk(admin_id, 1).await, mk(bob_id, 2).await);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = e.raw.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let admin_token: String = {
        let (_, v) = send(
            &e.raw,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"admin","password":common::ADMIN_PASSWORD})),
        )
        .await;
        v["token"].as_str().unwrap().to_string()
    };

    // unauthenticated upgrade is refused
    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == 401),
        "{err:?}"
    );
    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?token=bogus"))
        .await
        .unwrap_err();
    assert!(matches!(err, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == 401));

    let (mut ws_bob, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?token={btoken}"))
            .await
            .unwrap();
    let (mut ws_admin, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?token={admin_token}"))
            .await
            .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let progress = |id: i64, tmdb: i64| ServerEvent::RequestProgress {
        request_id: id,
        tmdb_id: tmdb,
        state: RequestState::Downloading,
        progress: 0.5,
        eta_seconds: None,
        speed_bps: None,
    };
    e.state
        .events
        .send(ServerEvent::ScanProgress {
            library_id: 1,
            scanned: 1,
            total: 2,
        })
        .unwrap();
    e.state.events.send(progress(r_admin, 1)).unwrap();
    e.state.events.send(progress(r_bob, 2)).unwrap();
    // sentinel so we know when to stop reading
    e.state
        .events
        .send(ServerEvent::ScanProgress {
            library_id: 99,
            scanned: 0,
            total: 0,
        })
        .unwrap();

    async fn drain<S>(ws: &mut S, until_sentinel: bool) -> Vec<Value>
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        let mut out = vec![];
        let end =
            tokio::time::Instant::now() + Duration::from_secs(if until_sentinel { 5 } else { 1 });
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(end, ws.next()).await {
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                let done = v["library_id"] == 99;
                out.push(v);
                if done && until_sentinel {
                    break;
                }
            }
        }
        out
    }
    let admin_events = drain(&mut ws_admin, true).await;
    let types: Vec<(&str, Option<i64>)> = admin_events
        .iter()
        .map(|v| (v["type"].as_str().unwrap(), v["request_id"].as_i64()))
        .collect();
    assert_eq!(
        types,
        [
            ("scan_progress", None),
            ("request_progress", Some(r_admin)),
            ("request_progress", Some(r_bob)),
            ("scan_progress", None)
        ]
    );
    // bob only gets his own request's progress, never scan events
    let bob_events = drain(&mut ws_bob, false).await;
    assert_eq!(bob_events.len(), 1, "{bob_events:?}");
    assert_eq!(bob_events[0]["type"], "request_progress");
    assert_eq!(bob_events[0]["request_id"], r_bob);
    assert_eq!(bob_events[0]["state"], "downloading");
}

#[tokio::test]
async fn requests_are_scoped_to_their_owner_and_security_headers_present() {
    let e = env().await;
    let (btoken, bob_id) = invited_user(&e, "bob", &[], json!({})).await;
    let bob = as_token(&e, &btoken);
    sqlx::query("INSERT INTO requests (user_id, media_type, tmdb_id, status) VALUES (1, 'movie', 10, 'searching'), (?, 'movie', 11, 'searching')")
        .bind(bob_id)
        .execute(&e.pool)
        .await
        .unwrap();
    let (_, mine) = get(&bob, "/api/v1/requests").await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["tmdb_id"], 11);
    assert_eq!(
        get(&e.admin, "/api/v1/requests")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, active) = get(&bob, "/api/v1/requests/active").await;
    assert_eq!(active.as_array().unwrap().len(), 1);
    assert_eq!(active[0]["tmdb_id"], 11);
    assert_eq!(
        get(&e.admin, "/api/v1/requests/active")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // retrying somebody else's request is a 404 for non-admins
    let admin_req: i64 = sqlx::query_scalar("SELECT id FROM requests WHERE tmdb_id = 10")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(
        send(
            &bob,
            "POST",
            &format!("/api/v1/requests/{admin_req}/retry"),
            None
        )
        .await
        .0,
        404
    );

    // hardening headers on every response (incl. errors); same-origin by default (no CORS headers)
    let (_, h, _) = send_h(
        &e.raw,
        "GET",
        "/api/v1/health",
        None,
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert!(h.get("access-control-allow-origin").is_none());
    let (s, h, _) = send_h(&e.raw, "GET", "/api/v1/me", None, &[]).await;
    assert_eq!(s, 401);
    assert_eq!(h["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn cors_allowlist_and_body_limit() {
    let e = env_with(
        |c| {
            c.cors_origins = vec!["https://app.example.com".into()];
            c.max_body_bytes = 2048;
        },
        None,
    )
    .await;
    let (_, h, _) = send_h(
        &e.raw,
        "GET",
        "/api/v1/health",
        None,
        &[("origin", "https://app.example.com")],
    )
    .await;
    assert_eq!(h["access-control-allow-origin"], "https://app.example.com");
    let (_, h, _) = send_h(
        &e.raw,
        "GET",
        "/api/v1/health",
        None,
        &[("origin", "https://evil.example")],
    )
    .await;
    assert!(h.get("access-control-allow-origin").is_none());
    // oversized bodies are refused
    let big = json!({"username": "a".repeat(5000), "password": "x"});
    let (s, _) = send(&e.raw, "POST", "/api/v1/auth/login", Some(big)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

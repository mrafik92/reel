//! TV sign-in by code (#147).
mod common;

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{AuthOptions, NewUser, Role, password::HashParams};
use serde_json::{Value, json};
use tower::ServiceExt;

const PW: &str = "correct horse battery";

struct Env {
    state: AppState,
    raw: Router,
    admin: String,
    _dir: tempfile::TempDir,
}

async fn env(ttl: Duration, interval: Duration) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let c = reel_core::Config {
        data_dir: dir.path().join("data"),
        public_url: Some("https://reel.example".into()),
        ..Default::default()
    };
    let state = AppState::new(pool, c).with_auth_options(AuthOptions {
        hash: HashParams {
            memory_kib: Some(1024),
            iterations: Some(1),
        },
        max_concurrent_hashes: 64,
        device_code_ttl: ttl,
        device_poll_interval: interval,
        ..Default::default()
    });
    let admin = common::admin_token(&state).await;
    Env {
        raw: app(state.clone()),
        state,
        admin,
        _dir: dir,
    }
}

async fn quick() -> Env {
    env(Duration::from_secs(600), Duration::ZERO).await
}

async fn post(e: &Env, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = e
        .raw
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let s = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn start(e: &Env) -> (String, String) {
    let (s, b) = post(
        e,
        "/api/v1/auth/device/start",
        None,
        json!({"device_id": "tv-1", "device_name": "Living Room TV", "client": "webos"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    (
        b["device_code"].as_str().unwrap().to_string(),
        b["user_code"].as_str().unwrap().to_string(),
    )
}

async fn poll(e: &Env, device_code: &str) -> (StatusCode, Value) {
    post(
        e,
        "/api/v1/auth/device/poll",
        None,
        json!({"device_code": device_code}),
    )
    .await
}

async fn approve(e: &Env, token: &str, code: &str) -> StatusCode {
    post(
        e,
        "/api/v1/me/device-link/approve",
        Some(token),
        json!({"user_code": code}),
    )
    .await
    .0
}

#[tokio::test]
async fn the_link_points_at_the_public_address() {
    let e = quick().await;
    let (_, b) = post(&e, "/api/v1/auth/device/start", None, json!({})).await;
    let code = b["user_code"].as_str().unwrap();
    assert_eq!(b["verification_uri"], "https://reel.example/link");
    assert_eq!(
        b["verification_uri_complete"],
        format!("https://reel.example/link?code={code}")
    );
}

#[tokio::test]
async fn an_unapproved_code_gets_no_session() {
    let e = quick().await;
    let (dc, _) = start(&e).await;
    let (s, b) = poll(&e, &dc).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(b["status"], "pending");
    assert!(b.get("token").is_none());
}

#[tokio::test]
async fn approving_needs_a_signed_in_user() {
    let e = quick().await;
    let (dc, code) = start(&e).await;
    let (s, _) = post(
        &e,
        "/api/v1/me/device-link/approve",
        None,
        json!({"user_code": code}),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(poll(&e, &dc).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn the_approvers_account_gets_its_own_revocable_session_once() {
    let e = quick().await;
    e.state
        .auth
        .create_user(NewUser {
            username: "maya".into(),
            password: PW.into(),
            role: Role::User,
            limits: Default::default(),
            library_ids: vec![],
        })
        .await
        .unwrap();
    let maya = e
        .state
        .auth
        .login("maya", PW, Default::default())
        .await
        .unwrap();
    let (dc, code) = start(&e).await;
    let (s, info) = post(
        &e,
        "/api/v1/me/device-link/lookup",
        Some(&maya.token),
        json!({"user_code": code.to_lowercase()}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "typed in lowercase: {info}");
    assert_eq!(info["device_name"], "Living Room TV");
    assert_eq!(
        approve(&e, &maya.token, &code).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        approve(&e, &e.admin, &code).await,
        StatusCode::CONFLICT,
        "a second person can't take over an approved code"
    );

    let (s, tv) = poll(&e, &dc).await;
    assert_eq!(s, StatusCode::OK, "{tv}");
    assert_eq!(tv["user"]["username"], "maya");
    let tv_token = tv["token"].as_str().unwrap();
    assert_ne!(tv_token, maya.token);

    assert_eq!(poll(&e, &dc).await.0, StatusCode::NOT_FOUND, "single use");
    assert_eq!(approve(&e, &maya.token, &code).await, StatusCode::NOT_FOUND);

    let list = e.state.auth.list_sessions(maya.user.id).await.unwrap();
    let row = list
        .iter()
        .find(|s| s.device_name.as_deref() == Some("Living Room TV"))
        .expect("the TV is in the sessions list");
    e.state
        .auth
        .revoke_session(maya.user.id, row.id)
        .await
        .unwrap();
    assert!(e.state.auth.authenticate(tv_token).await.is_err());
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let e = env(Duration::from_millis(30), Duration::ZERO).await;
    let (dc, code) = start(&e).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(approve(&e, &e.admin, &code).await, StatusCode::NOT_FOUND);
    assert_eq!(poll(&e, &dc).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn polling_too_fast_is_slowed_down() {
    let e = env(Duration::from_secs(600), Duration::from_secs(5)).await;
    let (dc, _) = start(&e).await;
    assert_eq!(poll(&e, &dc).await.0, StatusCode::ACCEPTED);
    assert_eq!(poll(&e, &dc).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_refused_code_stops_working() {
    let e = quick().await;
    let (dc, code) = start(&e).await;
    let (s, _) = post(
        &e,
        "/api/v1/me/device-link/deny",
        Some(&e.admin),
        json!({"user_code": code}),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(poll(&e, &dc).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn guessing_codes_is_rate_limited() {
    let e = quick().await;
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = post(
            &e,
            "/api/v1/me/device-link/lookup",
            Some(&e.admin),
            json!({"user_code": "ZZZZ-ZZZZ"}),
        )
        .await
        .0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn an_approval_dies_when_the_approver_is_signed_out_everywhere() {
    let e = quick().await;
    let (dc, code) = start(&e).await;
    assert_eq!(approve(&e, &e.admin, &code).await, StatusCode::NO_CONTENT);
    e.state.auth.revoke_all_sessions(1).await.unwrap();
    assert_eq!(poll(&e, &dc).await.0, StatusCode::NOT_FOUND);
}

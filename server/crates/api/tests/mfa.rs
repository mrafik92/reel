//! Second sign-in factor (#146): TOTP, recovery codes, lockout and the admin requirement.
mod common;

use std::{net::SocketAddr, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{AuthOptions, LimiterConfig, password::HashParams, signing::now_unix, totp};
use serde_json::{Value, json};
use tower::ServiceExt;

const PW: &str = "correct horse battery";
const LAN: &str = "192.168.1.2";
const PUBLIC: &str = "203.0.113.7";
const EDGE: &str = "10.10.10.73";

struct Env {
    state: AppState,
    raw: Router,
    _dir: tempfile::TempDir,
}

async fn env(user_max: u32, require: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let c = reel_core::Config {
        data_dir: dir.path().join("data"),
        trusted_proxies: vec![EDGE.into()],
        ..Default::default()
    };
    let state = AppState::new(pool, c).with_auth_options(AuthOptions {
        hash: HashParams {
            memory_kib: Some(1024),
            iterations: Some(1),
        },
        limiter: LimiterConfig {
            user_max_failures: user_max,
            ip_max_failures: 500,
            base_lock: Duration::from_secs(30),
            max_lock: Duration::from_secs(60),
            decay: Duration::from_secs(300),
        },
        max_concurrent_hashes: 64,
        ..Default::default()
    });
    state.auth.set_require_admin_2fa(require);
    state.auth.setup("admin", PW).await.unwrap();
    Env {
        raw: app(state.clone()),
        state,
        _dir: dir,
    }
}

/// A request from `client`, through the trusted edge proxy.
async fn call(
    e: &Env,
    client: &str,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-forwarded-for", client);
    if let Some(t) = token {
        r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = r.body(b).unwrap();
    let addr = SocketAddr::new(EDGE.parse().unwrap(), 50000);
    req.extensions_mut().insert(ConnectInfo(addr));
    let res = e.raw.clone().oneshot(req).await.unwrap();
    let s = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn password_login(e: &Env, client: &str) -> (StatusCode, Value) {
    call(
        e,
        client,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": "admin", "password": PW})),
    )
    .await
}

fn decode_base32(s: &str) -> Vec<u8> {
    let (mut out, mut buf, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => panic!("not base32: {c}"),
        };
        buf = (buf << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

/// Enroll TOTP from the LAN: `(secret, recovery codes, session token)`.
async fn enroll(e: &Env) -> (Vec<u8>, Vec<String>, String) {
    let (s, login) = password_login(e, LAN).await;
    assert_eq!(s, StatusCode::OK, "{login}");
    let token = login["token"].as_str().unwrap().to_string();
    let (s, st) = call(
        e,
        LAN,
        "POST",
        "/api/v1/me/totp/start",
        Some(&token),
        Some(json!({"password": PW})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{st}");
    let secret = decode_base32(st["secret"].as_str().unwrap());
    let (s, done) = call(
        e,
        LAN,
        "POST",
        "/api/v1/me/totp/confirm",
        Some(&token),
        Some(json!({"code": totp::code_for_tests(&secret, now_unix())})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{done}");
    let codes = done["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    (secret, codes, token)
}

async fn challenge(e: &Env) -> String {
    let (s, c) = password_login(e, LAN).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{c}");
    c["mfa_token"].as_str().unwrap().to_string()
}

async fn second_step(e: &Env, body: Value) -> (StatusCode, Value) {
    call(e, LAN, "POST", "/api/v1/auth/login/mfa", None, Some(body)).await
}

/// A code the replay guard hasn't seen: the next step, inside the drift window.
fn next_code(secret: &[u8]) -> String {
    totp::code_for_tests(secret, now_unix() + totp::STEP_SECS)
}

#[tokio::test]
async fn login_needs_the_code_and_a_code_works_only_once() {
    let e = env(50, false).await;
    let (secret, _, _) = enroll(&e).await;

    let t = challenge(&e).await;
    let (s, _) = call(&e, LAN, "GET", "/api/v1/me", Some(&t), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a challenge is not a session");

    let (s, _) = second_step(&e, json!({"mfa_token": t, "code": "000000x"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let code = next_code(&secret);
    let (s, ok) = second_step(&e, json!({"mfa_token": t, "code": code})).await;
    assert_eq!(s, StatusCode::OK, "{ok}");
    assert!(ok["token"].is_string());

    let (s, _) = second_step(&e, json!({"mfa_token": t, "code": code})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the challenge is single use");
    let t2 = challenge(&e).await;
    let (s, _) = second_step(&e, json!({"mfa_token": t2, "code": code})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the code can't be replayed");
}

#[tokio::test]
async fn the_enrollment_code_cannot_also_sign_in() {
    let e = env(50, false).await;
    let (secret, _, _) = enroll(&e).await;
    let t = challenge(&e).await;
    let used = totp::code_for_tests(&secret, now_unix());
    let (s, _) = second_step(&e, json!({"mfa_token": t, "code": used})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_recovery_code_works_once() {
    let e = env(50, false).await;
    let (_, codes, _) = enroll(&e).await;
    assert_eq!(codes.len(), 10);
    let t = challenge(&e).await;
    let upper = codes[0].to_uppercase();
    let (s, b) = second_step(&e, json!({"mfa_token": t, "recovery_code": upper})).await;
    assert_eq!(s, StatusCode::OK, "typed in capitals: {b}");
    let token = b["token"].as_str().unwrap();
    let (_, sec) = call(&e, LAN, "GET", "/api/v1/me/security", Some(token), None).await;
    assert_eq!(sec["recovery_codes_left"], 9);

    let t = challenge(&e).await;
    let (s, _) = second_step(&e, json!({"mfa_token": t, "recovery_code": codes[0]})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "already used");
}

#[tokio::test]
async fn second_factor_failures_lock_the_account() {
    let e = env(3, false).await;
    let (secret, _, _) = enroll(&e).await;
    let t = challenge(&e).await;
    for _ in 0..3 {
        let (s, _) = second_step(&e, json!({"mfa_token": t, "code": "000000"})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _) = second_step(&e, json!({"mfa_token": t, "code": next_code(&secret)})).await;
    assert_eq!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "even the right code waits"
    );
}

#[tokio::test]
async fn a_challenge_dies_after_five_wrong_codes() {
    let e = env(50, false).await;
    let (secret, _, _) = enroll(&e).await;
    let t = challenge(&e).await;
    for _ in 0..5 {
        second_step(&e, json!({"mfa_token": t, "code": "000000"})).await;
    }
    let (s, b) = second_step(&e, json!({"mfa_token": t, "code": next_code(&secret)})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
    assert!(b["error"].as_str().unwrap().contains("expired"));
}

#[tokio::test]
async fn an_admin_without_a_factor_has_no_powers_until_enrolling() {
    let e = env(50, true).await;
    let (_, login) = password_login(&e, LAN).await;
    assert_eq!(login["user"]["admin_powers"], false);
    assert_eq!(login["user"]["must_enroll_second_factor"], true);
    let token = login["token"].as_str().unwrap().to_string();
    let (s, b) = call(&e, LAN, "GET", "/api/v1/admin/users", Some(&token), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(b["error"].as_str().unwrap().contains("Settings"), "{b}");

    let (_, st) = call(
        &e,
        LAN,
        "POST",
        "/api/v1/me/totp/start",
        Some(&token),
        Some(json!({"password": PW})),
    )
    .await;
    let secret = decode_base32(st["secret"].as_str().unwrap());
    let (s, _) = call(
        &e,
        LAN,
        "POST",
        "/api/v1/me/totp/confirm",
        Some(&token),
        Some(json!({"code": totp::code_for_tests(&secret, now_unix())})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&e, LAN, "GET", "/api/v1/admin/users", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK, "the same session now has admin powers");

    let (s, b) = call(
        &e,
        LAN,
        "POST",
        "/api/v1/me/totp/remove",
        Some(&token),
        Some(json!({"password": PW})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "an admin keeps one factor: {b}");
}

#[tokio::test]
async fn an_admin_changes_sign_in_methods_only_from_home() {
    let e = env(50, true).await;
    let (_, login) = password_login(&e, PUBLIC).await;
    let token = login["token"].as_str().unwrap();
    let (s, b) = call(
        &e,
        PUBLIC,
        "POST",
        "/api/v1/me/totp/start",
        Some(token),
        Some(json!({"password": PW})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
}

#[tokio::test]
async fn enrolling_needs_the_password_and_a_wrong_one_is_not_a_401() {
    let e = env(50, false).await;
    let (_, login) = password_login(&e, LAN).await;
    let token = login["token"].as_str().unwrap();
    let (s, _) = call(
        &e,
        LAN,
        "POST",
        "/api/v1/me/totp/start",
        Some(token),
        Some(json!({"password": "not the password"})),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a 401 would sign the client out"
    );
}

#[tokio::test]
async fn the_secret_is_encrypted_at_rest_and_codes_are_hashed() {
    let e = env(50, false).await;
    let (secret, codes, _) = enroll(&e).await;
    let stored: Vec<u8> = sqlx::query_scalar("SELECT secret FROM user_totp")
        .fetch_one(&e.state.db)
        .await
        .unwrap();
    assert!(!stored.windows(secret.len()).any(|w| w == secret));
    let hashes: Vec<String> = sqlx::query_scalar("SELECT code_hash FROM user_recovery_codes")
        .fetch_all(&e.state.db)
        .await
        .unwrap();
    assert!(!hashes.iter().any(|h| codes.contains(h)));
}

#[tokio::test]
async fn an_admin_reset_removes_factors_and_signs_the_user_out() {
    let e = env(50, false).await;
    let (_, _, token) = enroll(&e).await;
    let (s, _) = call(
        &e,
        LAN,
        "DELETE",
        "/api/v1/admin/users/1/second-factor",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&e, LAN, "GET", "/api/v1/me", Some(&token), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, b) = password_login(&e, LAN).await;
    assert_eq!(s, StatusCode::OK, "password alone again: {b}");
}

#[test]
fn the_admin_requirement_is_on_by_default() {
    assert!(reel_core::Config::default().require_admin_2fa);
}

//! Provider sign-in against a mock issuer (#149): the full code flow with RS256 tokens.
mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use http_body_util::BodyExt;
use openssl::{hash::MessageDigest, pkey::PKey, rsa::Rsa, sign::Signer};
use reel_api::{AppState, app};
use reel_auth::{AuthOptions, NewOidcProvider, password::HashParams, signing::now_unix};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

const BINDING: &str = "browser-secret-kept-in-session-storage";
const CLIENT_ID: &str = "reel-client-id";
const CLIENT_SECRET: &str = "provider-client-secret-value";

struct Env {
    state: AppState,
    raw: Router,
    admin: String,
    issuer: MockServer,
    key: PKey<openssl::pkey::Private>,
    _dir: tempfile::TempDir,
}

fn binding_hash(b: &str) -> String {
    B64.encode(Sha256::digest(b.as_bytes()))
}

async fn env() -> Env {
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
        ..Default::default()
    });
    let admin = common::admin_token(&state).await;

    let issuer = MockServer::start().await;
    let rsa = Rsa::generate(2048).unwrap();
    let jwk = json!({
        "kty": "RSA", "kid": "k1", "alg": "RS256", "use": "sig",
        "n": B64.encode(rsa.n().to_vec()), "e": B64.encode(rsa.e().to_vec()),
    });
    let base = issuer.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "jwks_uri": format!("{base}/jwks"),
        })))
        .mount(&issuer)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [jwk]})))
        .mount(&issuer)
        .await;
    state
        .auth
        .create_oidc_provider(NewOidcProvider {
            slug: "mock".into(),
            name: "Mock".into(),
            issuer: base.clone(),
            discovery_url: format!("{base}/.well-known/openid-configuration"),
            client_id: CLIENT_ID.into(),
            client_secret: CLIENT_SECRET.into(),
        })
        .await
        .unwrap();
    Env {
        raw: app(state.clone()),
        state,
        admin,
        issuer,
        key: PKey::from_rsa(rsa).unwrap(),
        _dir: dir,
    }
}

async fn call(
    e: &Env,
    m: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, Option<String>) {
    let mut r = Request::builder().method(m).uri(uri);
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
    let res = e.raw.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let s = res.status();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        s,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        loc,
    )
}

fn sign(key: &PKey<openssl::pkey::Private>, claims: &Value) -> String {
    let msg = format!(
        "{}.{}",
        B64.encode(json!({"alg": "RS256", "kid": "k1"}).to_string()),
        B64.encode(claims.to_string())
    );
    let mut s = Signer::new(MessageDigest::sha256(), key).unwrap();
    s.update(msg.as_bytes()).unwrap();
    format!("{msg}.{}", B64.encode(s.sign_to_vec().unwrap()))
}

fn claims(e: &Env, sub: &str, nonce: &str) -> Value {
    json!({
        "iss": e.issuer.uri(), "aud": CLIENT_ID, "sub": sub, "nonce": nonce,
        "exp": now_unix() + 300, "iat": now_unix(), "email": "admin@reel.example",
    })
}

struct Started {
    state: String,
    nonce: String,
    code: String,
}

fn parse_start(url: &str) -> Started {
    let u = reqwest::Url::parse(url).unwrap();
    let get = |k: &str| {
        u.query_pairs()
            .find(|(n, _)| n == k)
            .unwrap_or_else(|| panic!("{k} missing from {url}"))
            .1
            .to_string()
    };
    assert_eq!(get("response_type"), "code");
    assert_eq!(get("code_challenge_method"), "S256");
    assert_eq!(
        get("redirect_uri"),
        "https://reel.example/api/v1/auth/oidc/callback"
    );
    assert!(get("scope").contains("openid"));
    Started {
        code: format!("code-{}", &get("state")[..12]),
        state: get("state"),
        nonce: get("nonce"),
    }
}

async fn start(e: &Env, purpose: &str) -> Started {
    let (s, b, _) = call(
        e,
        "POST",
        "/api/v1/auth/oidc/mock/start",
        None,
        Some(json!({"binding_hash": binding_hash(BINDING), "purpose": purpose})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    parse_start(b["authorization_url"].as_str().unwrap())
}

/// The provider answers this flow's code with `token`.
async fn provider_returns(e: &Env, f: &Started, token: String) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("code={}", f.code)))
        .and(body_string_contains("code_verifier="))
        .and(body_string_contains(
            "client_secret=provider-client-secret-value",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id_token": token})))
        .mount(&e.issuer)
        .await;
}

/// Follow the callback; returns the redirect's fragment.
async fn come_back(e: &Env, f: &Started) -> String {
    let uri = format!(
        "/api/v1/auth/oidc/callback?code={}&state={}",
        f.code, f.state
    );
    let (s, _, loc) = call(e, "GET", &uri, None, None).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let loc = loc.expect("redirect");
    assert!(loc.starts_with("https://reel.example/login/oidc#"), "{loc}");
    loc.split_once('#').unwrap().1.to_string()
}

async fn ticket_for(e: &Env, sub: &str, purpose: &str) -> String {
    let f = start(e, purpose).await;
    let token = sign(&e.key, &claims(e, sub, &f.nonce));
    provider_returns(e, &f, token).await;
    let frag = come_back(e, &f).await;
    frag.strip_prefix("ticket=")
        .unwrap_or_else(|| panic!("no ticket: {frag}"))
        .to_string()
}

async fn exchange(e: &Env, ticket: &str, binding: &str) -> (StatusCode, Value) {
    let (s, b, _) = call(
        e,
        "POST",
        "/api/v1/auth/oidc/exchange",
        None,
        Some(json!({"ticket": ticket, "binding": binding})),
    )
    .await;
    (s, b)
}

async fn link(e: &Env, sub: &str) {
    let (s, b, _) = call(
        e,
        "POST",
        "/api/v1/me/identities/mock/start",
        Some(&e.admin),
        Some(json!({"password": common::ADMIN_PASSWORD, "binding_hash": binding_hash(BINDING)})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let f = parse_start(b["authorization_url"].as_str().unwrap());
    provider_returns(e, &f, sign(&e.key, &claims(e, sub, &f.nonce))).await;
    let frag = come_back(e, &f).await;
    let (s, b, _) = call(
        e,
        "POST",
        "/api/v1/me/identities/complete",
        Some(&e.admin),
        Some(json!({"ticket": frag.strip_prefix("ticket=").unwrap(), "binding": BINDING})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
}

#[tokio::test]
async fn a_linked_identity_gets_a_normal_session() {
    let e = env().await;
    link(&e, "subject-admin").await;
    let t = ticket_for(&e, "subject-admin", "login").await;
    let (s, b) = exchange(&e, &t, BINDING).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["user"]["username"], "admin");
    let token = b["token"].as_str().unwrap();
    assert_eq!(
        call(&e, "GET", "/api/v1/me", Some(token), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        exchange(&e, &t, BINDING).await.0,
        StatusCode::UNAUTHORIZED,
        "single use"
    );
}

#[tokio::test]
async fn an_unlinked_identity_cannot_sign_in_even_with_a_matching_email() {
    let e = env().await;
    link(&e, "subject-admin").await;
    // same email claim as the linked identity, different subject
    let t = ticket_for(&e, "someone-else", "login").await;
    let (s, b) = exchange(&e, &t, BINDING).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
    assert!(b.get("token").is_none());
}

#[tokio::test]
async fn a_ticket_only_works_in_the_browser_that_started_it() {
    let e = env().await;
    link(&e, "subject-admin").await;
    let t = ticket_for(&e, "subject-admin", "login").await;
    assert_eq!(
        exchange(&e, &t, "another-browser").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        exchange(&e, &t, BINDING).await.0,
        StatusCode::OK,
        "not burned by the attempt"
    );
}

#[tokio::test]
async fn state_is_single_use() {
    let e = env().await;
    let f = start(&e, "login").await;
    provider_returns(&e, &f, sign(&e.key, &claims(&e, "s", &f.nonce))).await;
    assert!(come_back(&e, &f).await.starts_with("ticket="));
    assert_eq!(come_back(&e, &f).await, "error=expired");
}

#[tokio::test]
async fn bad_tokens_are_refused() {
    let e = env().await;
    let other = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let f = start(&e, "login").await;
    provider_returns(&e, &f, sign(&other, &claims(&e, "s", &f.nonce))).await;
    assert_eq!(come_back(&e, &f).await, "error=failed", "bad signature");

    let f = start(&e, "login").await;
    let mut c = claims(&e, "s", &f.nonce);
    c["aud"] = json!("another-app");
    provider_returns(&e, &f, sign(&e.key, &c)).await;
    assert_eq!(come_back(&e, &f).await, "error=failed", "wrong audience");

    let f = start(&e, "login").await;
    let mut c = claims(&e, "s", &f.nonce);
    c["iss"] = json!("https://evil.example");
    provider_returns(&e, &f, sign(&e.key, &c)).await;
    assert_eq!(come_back(&e, &f).await, "error=failed", "wrong issuer");

    let f = start(&e, "login").await;
    let mut c = claims(&e, "s", &f.nonce);
    c["exp"] = json!(now_unix() - 3600);
    provider_returns(&e, &f, sign(&e.key, &c)).await;
    assert_eq!(come_back(&e, &f).await, "error=failed", "expired");

    let f = start(&e, "login").await;
    provider_returns(&e, &f, sign(&e.key, &claims(&e, "s", "replayed-nonce"))).await;
    assert_eq!(come_back(&e, &f).await, "error=failed", "replayed nonce");
}

#[tokio::test]
async fn a_second_factor_is_still_required_after_the_provider() {
    let e = env().await;
    link(&e, "subject-admin").await;
    let admin = e.state.auth.get_user(1).await.unwrap().unwrap();
    e.state
        .auth
        .totp_start(&admin, common::ADMIN_PASSWORD)
        .await
        .unwrap();
    sqlx::query("UPDATE user_totp SET confirmed_at = 1")
        .execute(&e.state.db)
        .await
        .unwrap();
    let t = ticket_for(&e, "subject-admin", "login").await;
    let (s, b) = exchange(&e, &t, BINDING).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    assert!(b["mfa_token"].is_string() && b.get("token").is_none());
}

#[tokio::test]
async fn an_invite_can_be_accepted_with_a_provider_login() {
    let e = env().await;
    let (_, inv, _) = call(
        &e,
        "POST",
        "/api/v1/admin/invites",
        Some(&e.admin),
        Some(json!({})),
    )
    .await;
    let t = ticket_for(&e, "new-person", "invite").await;
    assert_eq!(
        exchange(&e, &t, BINDING).await.0,
        StatusCode::UNAUTHORIZED,
        "an invite ticket is not a login"
    );
    let (s, b, _) = call(
        &e,
        "POST",
        &format!(
            "/api/v1/auth/invite/{}/accept",
            inv["code"].as_str().unwrap()
        ),
        None,
        Some(json!({
            "username": "newperson", "password": "a calm violet harbour",
            "oidc_ticket": t, "oidc_binding": BINDING,
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let t = ticket_for(&e, "new-person", "login").await;
    let (s, b) = exchange(&e, &t, BINDING).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["user"]["username"], "newperson");
}

#[tokio::test]
async fn linking_needs_the_password() {
    let e = env().await;
    let (s, _, _) = call(
        &e,
        "POST",
        "/api/v1/me/identities/mock/start",
        Some(&e.admin),
        Some(json!({"password": "not the password", "binding_hash": binding_hash(BINDING)})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_client_secret_is_masked_and_encrypted() {
    let e = env().await;
    let (s, list, _) = call(
        &e,
        "GET",
        "/api/v1/admin/oidc/providers",
        Some(&e.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list[0]["secret_set"], true);
    assert!(!list.to_string().contains(CLIENT_SECRET));
    let stored: Vec<u8> = sqlx::query_scalar("SELECT client_secret FROM oidc_providers")
        .fetch_one(&e.state.db)
        .await
        .unwrap();
    assert!(
        !stored
            .windows(CLIENT_SECRET.len())
            .any(|w| w == CLIENT_SECRET.as_bytes())
    );
}

#[tokio::test]
async fn presets_pin_the_right_issuers() {
    let e = env().await;
    let (s, b, _) = call(
        &e,
        "POST",
        "/api/v1/admin/oidc/providers",
        Some(&e.admin),
        Some(json!({"preset": "microsoft", "client_id": "id", "client_secret": "secret"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(
        b["issuer"], "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0",
        "personal accounts only"
    );
    assert_eq!(
        b["redirect_uri"],
        "https://reel.example/api/v1/auth/oidc/callback"
    );
}

// ───────────── passwordless accounts (#287) ─────────────

const USER_PW: &str = "a quiet copper lantern";

/// A regular account with a password, signed in.
async fn regular_user(e: &Env) -> String {
    e.state
        .auth
        .create_user(reel_auth::NewUser {
            username: "maya".into(),
            password: USER_PW.into(),
            role: reel_auth::Role::User,
            limits: Default::default(),
            library_ids: vec![],
        })
        .await
        .unwrap();
    e.state
        .auth
        .login("maya", USER_PW, Default::default())
        .await
        .unwrap()
        .token
}

async fn link_as(e: &Env, token: &str, password: &str, sub: &str) -> StatusCode {
    let (s, b, _) = call(
        e,
        "POST",
        "/api/v1/me/identities/mock/start",
        Some(token),
        Some(json!({"password": password, "binding_hash": binding_hash(BINDING)})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let f = parse_start(b["authorization_url"].as_str().unwrap());
    provider_returns(e, &f, sign(&e.key, &claims(e, sub, &f.nonce))).await;
    let frag = come_back(e, &f).await;
    call(
        e,
        "POST",
        "/api/v1/me/identities/complete",
        Some(token),
        Some(json!({"ticket": frag.strip_prefix("ticket=").unwrap(), "binding": BINDING})),
    )
    .await
    .0
}

async fn password_login(e: &Env, user: &str, pw: &str) -> StatusCode {
    call(
        e,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": user, "password": pw})),
    )
    .await
    .0
}

async fn stored_hash(e: &Env, user: &str) -> String {
    sqlx::query_scalar("SELECT password_hash FROM users WHERE username = ?")
        .bind(user)
        .fetch_one(&e.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn linking_deletes_a_regular_users_password() {
    let e = env().await;
    let token = regular_user(&e).await;
    assert_eq!(password_login(&e, "maya", USER_PW).await, StatusCode::OK);
    assert_eq!(
        link_as(&e, &token, USER_PW, "maya-sub").await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        stored_hash(&e, "maya").await,
        reel_auth::password::NO_PASSWORD
    );
    assert_eq!(
        password_login(&e, "maya", USER_PW).await,
        StatusCode::UNAUTHORIZED
    );
    // the stand-in hash used for timing must not become a way in
    assert_eq!(
        password_login(&e, "maya", "reel-dummy-password").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        password_login(&e, "maya", "external").await,
        StatusCode::UNAUTHORIZED
    );

    let (_, me, _) = call(&e, "GET", "/api/v1/me", Some(&token), None).await;
    assert_eq!(me["has_password"], false);
    let t = ticket_for(&e, "maya-sub", "login").await;
    assert_eq!(
        exchange(&e, &t, BINDING).await.0,
        StatusCode::OK,
        "the provider still signs in"
    );
}

#[tokio::test]
async fn an_admin_keeps_the_password_until_there_is_an_emergency_passkey() {
    let e = env().await;
    let before = stored_hash(&e, "admin").await;
    link(&e, "subject-admin").await;
    assert_eq!(stored_hash(&e, "admin").await, before);
    assert_eq!(
        password_login(&e, "admin", common::ADMIN_PASSWORD).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_account_with_no_password_cannot_be_locked_out() {
    let e = env().await;
    let token = regular_user(&e).await;
    assert_eq!(
        link_as(&e, &token, USER_PW, "maya-sub").await,
        StatusCode::NO_CONTENT
    );

    let (_, list, _) = call(
        &e,
        "GET",
        "/api/v1/admin/oidc/providers",
        Some(&e.admin),
        None,
    )
    .await;
    let id = list[0]["id"].as_i64().unwrap();
    let uri = format!("/api/v1/admin/oidc/providers/{id}");
    let (s, b, _) = call(
        &e,
        "PATCH",
        &uri,
        Some(&e.admin),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "disable: {b}");
    let (s, b, _) = call(&e, "DELETE", &uri, Some(&e.admin), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "delete: {b}");

    // an admin can't give it a stored password back either
    let (_, users, _) = call(&e, "GET", "/api/v1/admin/users", Some(&e.admin), None).await;
    let maya = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "maya")
        .unwrap();
    let (s, b, _) = call(
        &e,
        "PATCH",
        &format!("/api/v1/admin/users/{}", maya["id"]),
        Some(&e.admin),
        Some(json!({"password": "a brand new long password"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    let (s, b, _) = call(
        &e,
        "POST",
        "/api/v1/me/password",
        Some(&token),
        Some(json!({"current_password": USER_PW, "new_password": "a brand new long password"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
}

#[tokio::test]
async fn an_invite_with_a_provider_stores_no_password() {
    let e = env().await;
    let (_, inv, _) = call(
        &e,
        "POST",
        "/api/v1/admin/invites",
        Some(&e.admin),
        Some(json!({})),
    )
    .await;
    let accept = format!(
        "/api/v1/auth/invite/{}/accept",
        inv["code"].as_str().unwrap()
    );
    let (s, b, _) = call(
        &e,
        "POST",
        &accept,
        None,
        Some(json!({"username": "newperson", "password": USER_PW})),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a password alone is refused: {b}"
    );

    let t = ticket_for(&e, "new-person", "invite").await;
    let (s, b, _) = call(
        &e,
        "POST",
        &accept,
        None,
        Some(json!({"username": "newperson", "oidc_ticket": t, "oidc_binding": BINDING})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["user"]["has_password"], false);
    assert_eq!(
        stored_hash(&e, "newperson").await,
        reel_auth::password::NO_PASSWORD
    );
}

#[tokio::test]
async fn login_options_follow_the_accounts() {
    let e = env().await;
    let (_, o, _) = call(&e, "GET", "/api/v1/auth/options", None, None).await;
    assert_eq!(o["providers"][0]["slug"], "mock");
    assert_eq!(o["password_login"], true, "the admin still has one");
    let (s, b, _) = call(
        &e,
        "POST",
        "/api/v1/auth/login/passkey",
        None,
        Some(json!({"username": "admin"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no passkey enrolled: {b}");
}

#[tokio::test]
async fn emergency_passkey_sign_in_is_refused_from_a_public_address() {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let c = reel_core::Config {
        data_dir: dir.path().join("data"),
        public_url: Some("https://reel.example".into()),
        trusted_proxies: vec!["10.10.10.73".into()],
        ..Default::default()
    };
    let state = AppState::new(pool, c);
    let raw = app(state);
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login/passkey")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", "203.0.113.7")
        .body(Body::from(json!({"username": "admin"}).to_string()))
        .unwrap();
    let addr = std::net::SocketAddr::new("10.10.10.73".parse().unwrap(), 50000);
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    let res = raw.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

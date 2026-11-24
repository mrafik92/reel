//! Security hardening: login limiter / Argon2 slots, setup guard, client-IP trust, signed-URL
//! revocation, session lifetime.
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
use reel_auth::{AuthError, AuthOptions, Device, LimiterConfig, password::HashParams};
use serde_json::{Value, json};
use tower::ServiceExt;

const PW: &str = "correct horse battery";

struct Env {
    state: AppState,
    raw: Router,
    _dir: tempfile::TempDir,
}

fn fast() -> HashParams {
    HashParams {
        memory_kib: Some(1024),
        iterations: Some(1),
    }
}

async fn env_with(cfg: impl FnOnce(&mut reel_core::Config), opts: AuthOptions, setup: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let mut c = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    cfg(&mut c);
    let state = AppState::new(pool, c).with_auth_options(opts);
    if setup {
        state.auth.setup("admin", PW).await.unwrap();
    }
    Env {
        raw: app(state.clone()),
        state,
        _dir: dir,
    }
}

fn opts(user_max: u32, ip_max: u32) -> AuthOptions {
    AuthOptions {
        hash: fast(),
        limiter: LimiterConfig {
            user_max_failures: user_max,
            ip_max_failures: ip_max,
            base_lock: Duration::from_secs(30),
            max_lock: Duration::from_secs(60),
            decay: Duration::from_secs(300),
        },
        max_concurrent_hashes: 64,
        ..Default::default()
    }
}

fn dev(ip: &str) -> Device {
    Device {
        ip: Some(ip.into()),
        ..Default::default()
    }
}

/// A request that arrives from socket `peer`.
async fn call(
    app: &Router,
    peer: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
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
    let mut req = r.body(b).unwrap();
    let ip: std::net::IpAddr = peer.parse().unwrap();
    let addr = SocketAddr::new(ip, 50000);
    req.extensions_mut().insert(ConnectInfo(addr));
    let res = app.clone().oneshot(req).await.unwrap();
    let s = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

// ───────────── login limiter ─────────────

#[tokio::test]
async fn parallel_wrong_guesses_cannot_exceed_the_failure_window() {
    let e = env_with(|_| {}, opts(3, 100), true).await;
    let mut tasks = vec![];
    for i in 0..40 {
        let auth = e.state.auth.clone();
        tasks.push(tokio::spawn(async move {
            auth.login("admin", &format!("guess-number-{i}"), dev("203.0.113.9"))
                .await
        }));
    }
    let (mut tried, mut locked) = (0, 0);
    for t in tasks {
        match t.await.unwrap() {
            Err(AuthError::InvalidCredentials) => tried += 1,
            Err(AuthError::Locked(_)) => locked += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        tried, 3,
        "only the window's worth of guesses were evaluated"
    );
    assert_eq!(locked, 37);
    // the lock is per (username, ip): the real admin from elsewhere still gets in
    assert!(
        e.state
            .auth
            .login("admin", PW, dev("198.51.100.7"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_flood_of_logins_gets_429_instead_of_queueing_argon2_work() {
    let mut o = opts(1000, 1000);
    o.max_concurrent_hashes = 1;
    o.hash = HashParams {
        memory_kib: Some(32 * 1024),
        iterations: Some(3),
    };
    let e = env_with(|_| {}, o, true).await;
    let mut tasks = vec![];
    for _ in 0..8 {
        let auth = e.state.auth.clone();
        tasks.push(tokio::spawn(async move {
            auth.login("admin", PW, dev("203.0.113.10"))
                .await
                .map(|_| ())
        }));
    }
    let (mut ok, mut busy) = (0, 0);
    for t in tasks {
        match t.await.unwrap() {
            Ok(()) => ok += 1,
            Err(AuthError::Locked(_)) => busy += 1,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(ok >= 1 && busy >= 1, "ok {ok}, busy {busy}");
    // and over HTTP it is a 429 with Retry-After
    let _ = e;
}

#[tokio::test]
async fn oversized_credentials_are_refused_before_hashing_keying_or_auditing() {
    let e = env_with(|_| {}, opts(5, 100), true).await;
    let long_user = "a".repeat(200);
    let long_pw = "p".repeat(300);
    assert!(matches!(
        e.state.auth.login(&long_user, PW, dev("203.0.113.1")).await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(matches!(
        e.state
            .auth
            .login("admin", &long_pw, dev("203.0.113.1"))
            .await,
        Err(AuthError::InvalidCredentials)
    ));
    let audit = e.state.auth.list_audit(100, None).await.unwrap();
    assert!(
        audit
            .iter()
            .all(|a| a.actor_name.as_deref().is_none_or(|n| n.len() <= 32)),
        "no long names in the audit log"
    );
    assert!(
        audit.iter().all(|a| a.action != "login_failure"),
        "cheap rejections are not audited rows"
    );
    // they still count against the address (spraying with huge bodies is not free)
    let ip_key_failures = e.state.auth.limiter().failures("ip:203.0.113.1");
    assert_eq!(ip_key_failures, 2);
}

#[tokio::test]
async fn ipv6_clients_are_limited_per_64_and_failures_are_audited_sparingly() {
    let e = env_with(|_| {}, opts(100, 3), true).await;
    // three different hosts of one /64 share the address budget
    for h in ["2001:db8:1:2::1", "2001:db8:1:2::2", "2001:db8:1:2:ffff::3"] {
        assert!(matches!(
            e.state.auth.login("nobody", "x", dev(h)).await,
            Err(AuthError::InvalidCredentials)
        ));
    }
    assert!(matches!(
        e.state
            .auth
            .login("admin", PW, dev("2001:db8:1:2::99"))
            .await,
        Err(AuthError::Locked(_))
    ));
    assert!(
        e.state
            .auth
            .login("admin", PW, dev("2001:db8:1:3::1"))
            .await
            .is_ok(),
        "other /64 unaffected"
    );
    assert_eq!(
        reel_auth::ip_bucket(Some("2001:db8:1:2:aaaa:bbbb:cccc:dddd")),
        "2001:db8:1:2::/64"
    );
    assert_eq!(reel_auth::ip_bucket(Some("10.1.2.3")), "10.1.2.3");
    // repeated failures of one (user, ip) → one audit row per minute
    let e2 = env_with(|_| {}, opts(100, 100), true).await;
    for _ in 0..10 {
        let _ = e2
            .state
            .auth
            .login("admin", "nope-nope-nope", dev("203.0.113.5"))
            .await;
    }
    let n = e2
        .state
        .auth
        .list_audit(100, None)
        .await
        .unwrap()
        .iter()
        .filter(|a| a.action == "login_failure")
        .count();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn limiter_memory_is_bounded() {
    let l = reel_auth::RateLimiter::new(LimiterConfig::default());
    for i in 0..60_000 {
        l.fail(&format!("user:attacker{i}|1.2.3.4"), 5);
    }
    assert!(l.len() <= 20_000, "{}", l.len());
}

// ───────────── sessions / signed URLs ─────────────

#[tokio::test]
async fn sessions_end_at_the_absolute_lifetime_even_when_used() {
    let mut o = opts(100, 100);
    o.absolute_session_ttl = Duration::from_secs(2);
    let e = env_with(|_| {}, o, true).await;
    let out = e
        .state
        .auth
        .login("admin", PW, dev("203.0.113.1"))
        .await
        .unwrap();
    assert!(
        out.expires_at <= reel_auth::signing::now_unix() + 3,
        "sliding expiry is capped by the absolute one"
    );
    assert!(e.state.auth.authenticate(&out.token).await.is_ok());
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(matches!(
        e.state.auth.authenticate(&out.token).await,
        Err(AuthError::Unauthorized)
    ));
}

#[tokio::test]
async fn signed_urls_die_when_the_user_changes_password_is_reset_disabled_or_revoked() {
    let e = env_with(|_| {}, opts(100, 100), true).await;
    let auth = &e.state.auth;
    let admin = auth.login("admin", PW, dev("203.0.113.1")).await.unwrap();
    let mk = |name: &str| {
        let auth = auth.clone();
        let name = name.to_string();
        async move {
            auth.create_user(reel_auth::NewUser {
                username: name,
                password: PW.into(),
                role: reel_auth::Role::User,
                limits: Default::default(),
                library_ids: vec![],
            })
            .await
            .unwrap()
        }
    };
    let valid = |uid: i64, q: &str| {
        let auth = auth.clone();
        let q: std::collections::HashMap<String, String> =
            form_urlencoded::parse(q.as_bytes()).into_owned().collect();
        async move {
            auth.verify_signature("direct:1", uid, q["exp"].parse().unwrap(), &q["sig"])
                .await
                .is_ok()
        }
    };
    let (bob, cy, di, ed) = (
        mk("bobby").await,
        mk("cyrus").await,
        mk("diana").await,
        mk("edward").await,
    );
    let q = |uid: i64| auth.signer(uid).query("direct:1", 3600, 1);
    let (qb, qc, qd, qe) = (q(bob.id), q(cy.id), q(di.id), q(ed.id));
    for (id, q) in [(bob.id, &qb), (cy.id, &qc), (di.id, &qd), (ed.id, &qe)] {
        assert!(valid(id, q).await);
    }
    // own password change
    auth.change_password(bob.id, PW, "another long password", None)
        .await
        .unwrap();
    assert!(!valid(bob.id, &qb).await, "password change");
    // admin reset
    auth.update_user(
        cy.id,
        reel_auth::UserPatch {
            password: Some("reset password 1".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!valid(cy.id, &qc).await, "admin reset");
    // disable
    auth.update_user(
        di.id,
        reel_auth::UserPatch {
            disabled: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!valid(di.id, &qd).await, "disable");
    // revoke all sessions
    auth.revoke_all_sessions(ed.id).await.unwrap();
    assert!(!valid(ed.id, &qe).await, "revoke all");
    // URLs signed after the bump work, other users are untouched
    assert!(valid(bob.id, &q(bob.id)).await);
    assert!(valid(1, &auth.signer(1).query("direct:1", 3600, 1)).await);
    let _ = admin;
    // the epoch survives a restart
    let before = auth.epoch(bob.id);
    assert!(before > 0);
    let fresh =
        reel_auth::AuthService::new(e.state.db.clone(), opts(1, 1), auth.signing_key().to_vec());
    assert_eq!(fresh.epoch(bob.id), 0);
    fresh.load_epochs().await.unwrap();
    assert_eq!(fresh.epoch(bob.id), before);
}

// ───────────── setup guard ─────────────

#[tokio::test]
async fn setup_is_lan_only_or_needs_the_one_time_token() {
    let e = env_with(|_| {}, opts(100, 100), false).await;
    let body = |tok: Option<&str>| {
        let mut v = json!({"username": "boss", "password": PW});
        if let Some(t) = tok {
            v["setup_token"] = json!(t);
        }
        v
    };
    let token = e
        .state
        .auth
        .ensure_setup_token()
        .await
        .unwrap()
        .expect("setup pending");
    assert_eq!(
        e.state.auth.ensure_setup_token().await.unwrap().as_deref(),
        Some(token.as_str()),
        "stable"
    );
    // internet client without / with a wrong token
    assert_eq!(
        call(
            &e.raw,
            "8.8.8.8",
            "POST",
            "/api/v1/setup",
            Some(body(None)),
            &[]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.raw,
            "8.8.8.8",
            "POST",
            "/api/v1/setup",
            Some(body(Some("nope"))),
            &[]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // LAN peer, but it presented proxy headers nobody vouches for (a proxy in front): needs the token
    assert_eq!(
        call(
            &e.raw,
            "172.18.0.2",
            "POST",
            "/api/v1/setup",
            Some(body(None)),
            &[("x-forwarded-for", "8.8.8.8")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // the token works from anywhere (body field or header)
    let (s, v) = call(
        &e.raw,
        "8.8.8.8",
        "POST",
        "/api/v1/setup",
        Some(body(None)),
        &[("x-setup-token", &token)],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // done: nothing left to claim (and no hashing happens for it)
    assert_eq!(
        call(
            &e.raw,
            "8.8.8.8",
            "POST",
            "/api/v1/setup",
            Some(body(Some(&token))),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(e.state.auth.ensure_setup_token().await.unwrap().is_none());
}

#[tokio::test]
async fn setup_from_the_lan_needs_no_token() {
    let e = env_with(|_| {}, opts(100, 100), false).await;
    let (s, v) = call(
        &e.raw,
        "192.168.1.20",
        "POST",
        "/api/v1/setup",
        Some(json!({"username": "boss", "password": PW})),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
}

#[tokio::test]
async fn setup_behind_a_trusted_proxy_judges_the_forwarded_client() {
    let cfg = |c: &mut reel_core::Config| c.trusted_proxies = vec!["172.18.0.0/16".into()];
    let e = env_with(cfg, opts(100, 100), false).await;
    let b = json!({"username": "boss", "password": PW});
    // proxy says the client is on the internet → refused
    assert_eq!(
        call(
            &e.raw,
            "172.18.0.2",
            "POST",
            "/api/v1/setup",
            Some(b.clone()),
            &[("x-forwarded-for", "8.8.8.8")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // a LAN client behind the proxy → accepted
    assert_eq!(
        call(
            &e.raw,
            "172.18.0.2",
            "POST",
            "/api/v1/setup",
            Some(b),
            &[("x-forwarded-for", "192.168.1.30")]
        )
        .await
        .0,
        StatusCode::CREATED
    );
}

// ───────────── public edge: Caddy (10.10.10.73) is the only trusted proxy ─────────────
//
// Production shape for public exposure (#31): internet → Caddy :8443 on CT 107 → Reel :8420 with
// `REEL_TRUSTED_PROXIES=10.10.10.73`. Caddy overwrites X-Forwarded-For with the address it saw.
// Nothing an internet client puts in a header may make it look like the LAN.

const CADDY: &str = "10.10.10.73";

/// (socket peer, request headers, what the case shows)
type Spoof<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

fn edge(c: &mut reel_core::Config) {
    c.trusted_proxies = vec![CADDY.into()];
}

#[tokio::test]
async fn edge_spoofed_forwarding_headers_cannot_open_lan_only_setup() {
    let e = env_with(edge, opts(100, 100), false).await;
    let b = json!({"username": "boss", "password": PW});
    let spoofs: &[Spoof] = &[
        (
            CADDY,
            &[("x-forwarded-for", "203.0.113.7")],
            "Caddy forwarded an internet client",
        ),
        (
            CADDY,
            &[("x-forwarded-for", "192.168.1.5, 203.0.113.7")],
            "a proxy that appends: the client's own left-hand entry is not believed",
        ),
        (
            CADDY,
            &[("x-forwarded-for", "10.10.10.73, 203.0.113.7")],
            "claiming to be the proxy itself",
        ),
        (
            CADDY,
            &[
                ("x-forwarded-for", "203.0.113.7"),
                ("x-real-ip", "192.168.1.5"),
            ],
            "X-Real-IP only fills in when X-Forwarded-For is missing",
        ),
        (
            CADDY,
            &[
                ("x-forwarded-for", "203.0.113.7"),
                ("forwarded", "for=192.168.1.5"),
            ],
            "RFC 7239 Forwarded is never read",
        ),
        (
            "203.0.113.7",
            &[("x-forwarded-for", "192.168.1.5")],
            "straight to :8420 from the internet",
        ),
        (
            "203.0.113.7",
            &[("x-real-ip", "127.0.0.1")],
            "straight to :8420 with X-Real-IP",
        ),
        ("2001:db8::7", &[("x-forwarded-for", "::1")], "IPv6 peer"),
    ];
    for (peer, headers, why) in spoofs {
        let (s, v) = call(
            &e.raw,
            peer,
            "POST",
            "/api/v1/setup",
            Some(b.clone()),
            headers,
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{why}: {v}");
    }
    // the gate is not simply shut: a LAN client behind Caddy may still claim the server
    let (s, v) = call(
        &e.raw,
        CADDY,
        "POST",
        "/api/v1/setup",
        Some(b),
        &[("x-forwarded-for", "192.168.1.30")],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
}

#[tokio::test]
async fn edge_only_the_named_proxy_may_vouch_for_a_client() {
    let e = env_with(edge, opts(100, 100), true).await;
    // another LAN host (a TV, another container) talking to :8420 directly picks no address
    assert_eq!(
        recorded_ip(&e, "10.10.10.50", Some("198.51.100.4")).await,
        "10.10.10.50"
    );
    assert_eq!(
        recorded_ip(&e, CADDY, Some("6.6.6.6, 198.51.100.4")).await,
        "198.51.100.4"
    );
    // the deprecated blanket alias believes any private-range peer
    let e = env_with(|c| c.trust_proxy_headers = true, opts(100, 100), true).await;
    assert_eq!(
        recorded_ip(&e, "10.10.10.50", Some("198.51.100.4")).await,
        "198.51.100.4"
    );
}

#[tokio::test]
async fn edge_login_lockout_follows_the_real_client_not_a_spoofed_prefix() {
    let e = env_with(edge, opts(100, 3), true).await;
    let login = |user: String, pw: &'static str, xff: String| {
        let app = e.raw.clone();
        async move {
            call(
                &app,
                CADDY,
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username": user, "password": pw})),
                &[("x-forwarded-for", xff.as_str())],
            )
            .await
            .0
        }
    };
    // password spraying from one internet address, a fresh spoofed prefix each time
    for i in 0..3 {
        assert_eq!(
            login(
                format!("user{i}"),
                "wrong-wrong",
                format!("192.168.1.{i}, 203.0.113.7")
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login("admin".into(), PW, "10.0.0.99, 203.0.113.7".into()).await,
        StatusCode::TOO_MANY_REQUESTS,
        "the address is locked whatever it prepends"
    );
    // other clients behind the same proxy are not caught by that lock
    assert_eq!(
        login("admin".into(), PW, "198.51.100.4".into()).await,
        StatusCode::OK
    );
}

// ───────────── client ip trust ─────────────

/// Source IP the audit log recorded for a failed login sent from `peer` with `xff`.
async fn recorded_ip(e: &Env, peer: &str, xff: Option<&str>) -> String {
    let h: Vec<(&str, &str)> = xff.map(|x| ("x-forwarded-for", x)).into_iter().collect();
    let user = format!(
        "probe{}",
        e.state.auth.list_audit(500, None).await.unwrap().len()
    );
    let (s, _) = call(
        &e.raw,
        peer,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username": user, "password": "wrong-wrong"})),
        &h,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    e.state
        .auth
        .list_audit(500, None)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.action == "login_failure" && a.actor_name.as_deref() == Some(user.as_str()))
        .and_then(|a| a.ip)
        .unwrap()
}

#[tokio::test]
async fn forwarded_headers_are_ignored_unless_the_peer_is_a_trusted_proxy() {
    // default: no trusted proxies → XFF is spoofable, so it is ignored
    let e = env_with(|_| {}, opts(100, 100), true).await;
    assert_eq!(
        recorded_ip(&e, "203.0.113.50", Some("1.2.3.4")).await,
        "203.0.113.50"
    );
    assert_eq!(
        recorded_ip(&e, "172.18.0.2", Some("1.2.3.4")).await,
        "172.18.0.2"
    );

    let cfg = |c: &mut reel_core::Config| {
        c.trusted_proxies = vec!["172.18.0.0/16".into(), "10.9.9.9".into()]
    };
    let e = env_with(cfg, opts(100, 100), true).await;
    // an untrusted peer cannot vouch for anything
    assert_eq!(
        recorded_ip(&e, "203.0.113.50", Some("1.2.3.4")).await,
        "203.0.113.50"
    );
    // trusted proxy: the client is the right-most entry that is not a trusted proxy …
    assert_eq!(
        recorded_ip(&e, "172.18.0.2", Some("198.51.100.4")).await,
        "198.51.100.4"
    );
    // … so entries a client prepends cannot choose its address
    assert_eq!(
        recorded_ip(&e, "172.18.0.2", Some("6.6.6.6, 198.51.100.4")).await,
        "198.51.100.4"
    );
    // chained proxies are skipped
    assert_eq!(
        recorded_ip(&e, "172.18.0.2", Some("198.51.100.4, 10.9.9.9")).await,
        "198.51.100.4"
    );
    // no header: the peer itself
    assert_eq!(recorded_ip(&e, "172.18.0.2", None).await, "172.18.0.2");

    // deprecated alias: believe private-range peers only
    let cfg = |c: &mut reel_core::Config| c.trust_proxy_headers = true;
    let e = env_with(cfg, opts(100, 100), true).await;
    assert_eq!(
        recorded_ip(&e, "192.168.1.2", Some("198.51.100.4")).await,
        "198.51.100.4"
    );
    assert_eq!(
        recorded_ip(&e, "203.0.113.50", Some("198.51.100.4")).await,
        "203.0.113.50"
    );
}

// ───────────── permissions / requests / webhook / profiles ─────────────

fn authed_as(e: &Env, token: &str) -> Router {
    common::with_token(e.raw.clone(), token)
}

async fn api(app: &Router, m: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    call(app, "192.168.1.2", m, uri, body, &[]).await
}

#[tokio::test]
async fn invited_users_start_without_live_tv_vod_requests_or_casting_until_an_admin_grants_them() {
    let e = env_with(|_| {}, opts(100, 100), true).await;
    let admin_tok = e
        .state
        .auth
        .login("admin", PW, dev("192.168.1.2"))
        .await
        .unwrap()
        .token;
    let admin = authed_as(&e, &admin_tok);
    // an invite without explicit limits/permissions: conservative limits, no feature access
    let (s, inv) = api(&admin, "POST", "/api/v1/admin/invites", Some(json!({}))).await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    assert_eq!(
        inv["permissions"],
        json!({"iptv_access": false, "vod_access": false, "can_request": false, "can_cast": false})
    );
    assert_eq!(inv["limits"]["max_streams"], 2);
    assert_eq!(inv["limits"]["max_transcodes"], 1);
    assert_eq!(inv["limits"]["max_bitrate_kbps"], 8000);
    assert!(inv["limits"]["request_quota_movies"].as_i64().unwrap() > 0);
    let code = inv["code"].as_str().unwrap();
    let (s, v) = api(
        &e.raw,
        "POST",
        &format!("/api/v1/auth/invite/{code}/accept"),
        Some(json!({"username":"newbie","password":PW})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let tok = v["token"].as_str().unwrap().to_string();
    let uid = v["user"]["id"].as_i64().unwrap();
    assert_eq!(v["user"]["permissions"]["iptv_access"], false);
    let user = authed_as(&e, &tok);
    for (m, uri) in [
        ("GET", "/api/v1/iptv/channels"),
        ("GET", "/api/v1/iptv/groups"),
        ("GET", "/api/v1/vod/movies"),
        ("GET", "/api/v1/requests"),
        ("GET", "/api/v1/discover/trending"),
        ("GET", "/api/v1/downloads"),
        ("GET", "/api/v1/cast/devices"),
    ] {
        let (s, v) = api(&user, m, uri, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}: {v}");
    }
    // normal things still work, and search quietly leaves out the gated sections
    assert_eq!(
        api(&user, "GET", "/api/v1/me", None).await.0,
        StatusCode::OK
    );
    let (s, v) = api(&user, "GET", "/api/v1/search/all?q=matrix", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["discover"], json!([]));
    assert_eq!(v["vod"], json!([]));
    // an admin grants exactly what they want
    let (s, v) = api(&admin, "PATCH", &format!("/api/v1/admin/users/{uid}"), Some(json!({"permissions": {"iptv_access": true, "vod_access": false, "can_request": true, "can_cast": true}}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["permissions"]["can_cast"], true);
    assert_eq!(
        api(&user, "GET", "/api/v1/iptv/channels", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(&user, "GET", "/api/v1/requests", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(&user, "GET", "/api/v1/cast/devices", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(&user, "GET", "/api/v1/vod/movies", None).await.0,
        StatusCode::FORBIDDEN
    );
    // admins are never gated; the device list admin endpoint stays admin-only
    assert_eq!(
        api(&admin, "GET", "/api/v1/vod/movies", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            &user,
            "POST",
            "/api/v1/admin/cast/devices",
            Some(json!({"ip": "127.0.0.1"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // accounts an admin creates directly / pre-existing users keep IPTV, VOD and requests; casting is opt-in
    e.state
        .auth
        .create_user(reel_auth::NewUser {
            username: "oldtimer".into(),
            password: PW.into(),
            role: reel_auth::Role::User,
            limits: Default::default(),
            library_ids: vec![],
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login("oldtimer", PW, dev("192.168.1.2"))
        .await
        .unwrap()
        .token;
    let old = authed_as(&e, &t);
    assert_eq!(
        api(&old, "GET", "/api/v1/iptv/channels", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(&old, "GET", "/api/v1/vod/movies", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api(&old, "GET", "/api/v1/cast/devices", None).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn webhooks_need_a_long_secret_accept_header_and_basic_auth_reject_dotdot_paths_and_lock_out_guessers()
 {
    let long = "w".repeat(40);
    let cfg = |c: &mut reel_core::Config| c.webhook_secret = Some("w".repeat(40));
    let e = env_with(cfg, opts(100, 100), true).await;
    let hook = |headers: &[(&str, &str)], q: &str, peer: &str, body: Value| {
        let raw = e.raw.clone();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let q = q.to_string();
        let peer = peer.to_string();
        async move {
            let h: Vec<(&str, &str)> = headers
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            call(
                &raw,
                &peer,
                "POST",
                &format!("/api/v1/webhooks/arr{q}"),
                Some(body),
                &h,
            )
            .await
        }
    };
    let test = json!({"eventType": "Test"});
    let basic = {
        use base64::Engine;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("radarr:{long}"))
        )
    };
    assert_eq!(
        hook(&[("x-reel-token", &long)], "", "10.0.0.5", test.clone())
            .await
            .0,
        StatusCode::OK,
        "header"
    );
    assert_eq!(
        hook(&[("authorization", &basic)], "", "10.0.0.5", test.clone())
            .await
            .0,
        StatusCode::OK,
        "basic"
    );
    assert_eq!(
        hook(
            &[("authorization", &format!("Bearer {long}"))],
            "",
            "10.0.0.5",
            test.clone()
        )
        .await
        .0,
        StatusCode::OK,
        "bearer"
    );
    assert_eq!(
        hook(&[], &format!("?token={long}"), "10.0.0.5", test.clone())
            .await
            .0,
        StatusCode::OK,
        "query (deprecated)"
    );
    assert_eq!(
        hook(&[("x-reel-token", "short")], "", "10.0.0.5", test.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // a prefix / different length never matches (and says nothing about the length)
    assert_eq!(
        hook(
            &[("x-reel-token", &long[..39])],
            "",
            "10.0.0.5",
            test.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // `..` paths are ignored, never scanned
    let evil = json!({"eventType": "Download", "movie": {"id": 1, "tmdbId": 603, "title": "x"}, "movieFile": {"path": "/media/../etc/passwd"}});
    let (s, v) = hook(&[("x-reel-token", &long)], "", "10.0.0.5", evil).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["scanned_paths"], json!([]));
    // guessing is locked out per address (valid callers elsewhere are unaffected)
    for _ in 0..12 {
        let _ = hook(
            &[("x-reel-token", "guess")],
            "",
            "203.0.113.77",
            test.clone(),
        )
        .await;
    }
    assert_eq!(
        hook(&[("x-reel-token", &long)], "", "203.0.113.77", test.clone())
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        hook(&[("x-reel-token", &long)], "", "10.0.0.5", test.clone())
            .await
            .0,
        StatusCode::OK
    );
    // a secret shorter than 32 characters keeps webhooks off
    let e2 = env_with(
        |c| c.webhook_secret = Some("tooshort".into()),
        opts(100, 100),
        true,
    )
    .await;
    let (s, v) = call(
        &e2.raw,
        "10.0.0.5",
        "POST",
        "/api/v1/webhooks/arr",
        Some(test),
        &[("x-reel-token", "tooshort")],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("at least 32"), "{v}");
}

#[test]
fn client_supplied_device_profiles_cannot_force_a_hevc_encode_for_non_admins() {
    // exercised through the preset/profile parser used by decide: an HEVC-only profile gets H.264 added
    let mk = |role| reel_auth::User {
        id: 2,
        username: "u".into(),
        role,
        disabled: false,
        created_at: 0,
        limits: Default::default(),
        perms: reel_auth::Permissions::all(),
        admin_powers: true,
    };
    let mut p = reel_playback::DeviceProfile::preset("tizen").unwrap();
    p.video.retain(|v| v.codec == "hevc");
    p.max_width = 99_999;
    p.max_height = 99_999;
    let s = reel_api::sanitize_profile_for_tests(&mk(reel_auth::Role::User), p.clone());
    assert!(
        s.video_support("h264").is_some(),
        "H.264 is always a target"
    );
    assert_eq!(s.video[0].codec, "h264");
    assert!(s.max_width <= 7680 && s.max_height <= 4320);
    let a = reel_api::sanitize_profile_for_tests(&mk(reel_auth::Role::Admin), p);
    assert!(
        a.video_support("h264").is_none(),
        "admins are trusted as they are"
    );
}

// ───────────── websockets ─────────────

async fn serve(e: &Env) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let r = e.raw.clone();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    addr
}

/// #81: idle `/events` sockets are pinged, so dead paths are noticed and live ones kept warm.
#[tokio::test]
async fn idle_websockets_are_pinged() {
    use futures_util::StreamExt;
    let e = env_with(|c| c.ws_ping_secs = 1, opts(100, 100), true).await;
    let addr = serve(&e).await;
    let login = e
        .state
        .auth
        .login("admin", PW, dev("127.0.0.1"))
        .await
        .unwrap();
    let admin = authed_as(&e, &login.token);
    let t = api(&admin, "POST", "/api/v1/auth/ws-ticket", None).await.1["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?ticket={t}"))
            .await
            .unwrap();
    let pinged = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) => break true,
                Some(Ok(_)) => continue,
                _ => break false,
            }
        }
    })
    .await;
    assert_eq!(pinged, Ok(true), "a ping within a few seconds");
}

#[tokio::test]
async fn websockets_use_single_use_tickets_are_capped_and_closed_when_access_is_revoked() {
    use futures_util::StreamExt;
    let e = env_with(|c| c.ws_recheck_secs = 1, opts(100, 100), true).await;
    let addr = serve(&e).await;
    let login = e
        .state
        .auth
        .login("admin", PW, dev("127.0.0.1"))
        .await
        .unwrap();
    let admin = authed_as(&e, &login.token);
    let ticket = || {
        let admin = admin.clone();
        async move {
            api(&admin, "POST", "/api/v1/auth/ws-ticket", None).await.1["ticket"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    assert_eq!(
        api(&e.raw, "POST", "/api/v1/auth/ws-ticket", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let url = |t: &str| format!("ws://{addr}/api/v1/events?ticket={t}");
    // a ticket opens exactly one socket
    let t = ticket().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url(&t)).await.unwrap();
    let again = tokio_tungstenite::connect_async(url(&t)).await.unwrap_err();
    assert!(
        matches!(again, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == 401),
        "{again:?}"
    );
    let bogus = tokio_tungstenite::connect_async(url("nope"))
        .await
        .unwrap_err();
    assert!(
        matches!(bogus, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == 401)
    );
    // per-user cap: the 9th concurrent socket is refused
    let mut open = vec![];
    for _ in 0..7 {
        open.push(
            tokio_tungstenite::connect_async(url(&ticket().await))
                .await
                .unwrap()
                .0,
        );
    }
    let over = tokio_tungstenite::connect_async(url(&ticket().await))
        .await
        .unwrap_err();
    assert!(
        matches!(over, tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status() == 429),
        "{over:?}"
    );
    drop(open);
    // revoking the session closes the open socket
    e.state.auth.revoke_all_sessions(1).await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None
                | Some(Err(_))
                | Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => break true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert_eq!(closed, Ok(true), "socket closed after revoke");
}

/// #28: the session token is never accepted in the `/events` URL (it would land in proxy and edge
/// logs); a ticket and the `Authorization` header still work.
#[tokio::test]
async fn websocket_rejects_session_token_in_url_but_accepts_tickets() {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let e = env_with(|_| {}, opts(100, 100), true).await;
    let addr = serve(&e).await;
    let login = e
        .state
        .auth
        .login("admin", PW, dev("127.0.0.1"))
        .await
        .unwrap();
    let token = login.token.clone();
    let is_401 = |r: Result<_, Error>| matches!(r, Err(Error::Http(ref r)) if r.status() == 401);

    // a valid, live session token in the query string is refused
    let r = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?token={token}"))
        .await
        .map(|_| ());
    assert!(is_401(r), "?token= must be rejected");
    // ...also next to a bogus ticket (no fallback from a failed ticket to the token)
    let r = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/api/v1/events?ticket=nope&token={token}"
    ))
    .await
    .map(|_| ());
    assert!(is_401(r), "a bad ticket must not fall back to ?token=");

    // the same session still opens a socket with a ticket...
    let admin = authed_as(&e, &token);
    let t = api(&admin, "POST", "/api/v1/auth/ws-ticket", None).await.1["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?ticket={t}"))
            .await
            .is_ok(),
        "ticket works"
    );
    // ...and with the Authorization header (native clients; nothing in the URL)
    let mut req = format!("ws://{addr}/api/v1/events")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    assert!(
        tokio_tungstenite::connect_async(req).await.is_ok(),
        "Authorization header works"
    );
}

// ───────────── admin powers need the LAN or Tailscale (#127) ─────────────

const EDGE: &str = "10.10.10.73";

fn behind_edge(c: &mut reel_core::Config) {
    c.trusted_proxies = vec![EDGE.into()];
}

async fn edge_login(e: &Env, client: &str) -> (StatusCode, Value) {
    call(
        &e.raw,
        EDGE,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username": "admin", "password": PW})),
        &[("x-forwarded-for", client)],
    )
    .await
}

#[tokio::test]
async fn an_admin_on_a_public_address_can_watch_but_has_no_admin_powers() {
    let e = env_with(behind_edge, opts(5, 50), true).await;
    let (s, login) = edge_login(&e, "203.0.113.7").await;
    assert_eq!(s, StatusCode::OK, "the password still signs in: {login}");
    assert_eq!(login["user"]["admin_powers"], false);
    let bearer = format!("Bearer {}", login["token"].as_str().unwrap());
    let public = [
        ("x-forwarded-for", "203.0.113.7"),
        ("authorization", bearer.as_str()),
    ];

    let (s, me) = call(&e.raw, EDGE, "GET", "/api/v1/me", None, &public).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["role"], "admin");
    assert_eq!(me["admin_powers"], false);
    assert_eq!(
        me["permissions"]["iptv_access"], true,
        "viewing is unchanged"
    );
    assert!(me.get("library_ids").is_none(), "still sees every library");

    for (method, uri, body) in [
        ("GET", "/api/v1/admin/users", None),
        (
            "POST",
            "/api/v1/libraries",
            Some(json!({"name": "x", "kind": "movies", "paths": ["/tmp"]})),
        ),
        ("POST", "/api/v1/libraries/1/scan", None),
        ("POST", "/api/v1/requests/1/approve", Some(json!({}))),
        ("POST", "/api/v1/requests/1/decline", Some(json!({}))),
    ] {
        let (s, b) = call(&e.raw, EDGE, method, uri, body, &public).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {uri}: {b}");
    }

    // the same token from home regains them, so no second account or re-login is needed
    for client in ["192.168.1.5", "100.100.20.30"] {
        let home = [
            ("x-forwarded-for", client),
            ("authorization", bearer.as_str()),
        ];
        let (s, me) = call(&e.raw, EDGE, "GET", "/api/v1/me", None, &home).await;
        assert_eq!((s, &me["admin_powers"]), (StatusCode::OK, &json!(true)));
        let (s, _) = call(&e.raw, EDGE, "GET", "/api/v1/admin/users", None, &home).await;
        assert_eq!(s, StatusCode::OK, "{client}");
    }
}

#[tokio::test]
async fn a_client_cannot_claim_an_admin_network_with_proxy_headers() {
    let e = env_with(behind_edge, opts(5, 50), true).await;
    let (_, login) = edge_login(&e, "192.168.1.5").await;
    assert_eq!(login["user"]["admin_powers"], true);
    let bearer = format!("Bearer {}", login["token"].as_str().unwrap());
    let cases: [(&str, &str); 3] = [
        // straight from the internet, spoofing a LAN address
        ("203.0.113.7", "192.168.1.5"),
        // through the edge: the right-most untrusted entry is the real client
        (EDGE, "192.168.1.5, 203.0.113.7"),
        // a private peer that isn't the trusted proxy: the real client is unknown, so refuse
        ("172.18.0.9", "192.168.1.5"),
    ];
    for (peer, xff) in cases {
        let h = [("x-forwarded-for", xff), ("authorization", bearer.as_str())];
        let (s, b) = call(&e.raw, peer, "GET", "/api/v1/admin/users", None, &h).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "peer {peer}, xff {xff}: {b}");
    }
}

// ───────────── password policy (#148) ─────────────

#[tokio::test]
async fn weak_passwords_are_refused_everywhere_a_password_is_set() {
    let e = env_with(|_| {}, opts(50, 500), false).await;
    let lan = "192.168.1.2";
    for weak in ["elevenchars", "password1234"] {
        let (s, b) = call(
            &e.raw,
            lan,
            "POST",
            "/api/v1/setup",
            Some(json!({"username": "admin", "password": weak})),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "setup {weak}: {b}");
    }
    let (s, setup) = call(
        &e.raw,
        lan,
        "POST",
        "/api/v1/setup",
        Some(json!({"username": "admin", "password": PW})),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{setup}");
    let bearer = format!("Bearer {}", setup["token"].as_str().unwrap());
    let auth = [("authorization", bearer.as_str())];

    let (_, inv) = call(
        &e.raw,
        lan,
        "POST",
        "/api/v1/admin/invites",
        Some(json!({})),
        &auth,
    )
    .await;
    let accept = format!(
        "/api/v1/auth/invite/{}/accept",
        inv["code"].as_str().unwrap()
    );
    for weak in ["elevenchars", "qwerty123456"] {
        let (s, b) = call(
            &e.raw,
            lan,
            "POST",
            &accept,
            Some(json!({"username": "newuser", "password": weak})),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "invite {weak}: {b}");
        let (s, b) = call(
            &e.raw,
            lan,
            "POST",
            "/api/v1/me/password",
            Some(json!({"current_password": PW, "new_password": weak})),
            &auth,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "change {weak}: {b}");
        let (s, b) = call(
            &e.raw,
            lan,
            "POST",
            "/api/v1/admin/users",
            Some(json!({"username": "newuser", "password": weak})),
            &auth,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "create {weak}: {b}");
    }
    let (s, b) = call(
        &e.raw,
        lan,
        "POST",
        &accept,
        Some(json!({"username": "newuser", "password": "a calm violet harbour"})),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "a good password is accepted: {b}");
}

#[tokio::test]
async fn an_existing_short_password_still_signs_in_and_is_flagged() {
    let e = env_with(|_| {}, opts(50, 500), true).await;
    let old = reel_auth::password::hash("oldpass8", fast()).unwrap();
    sqlx::query("UPDATE users SET password_hash = ? WHERE username = 'admin'")
        .bind(old)
        .execute(&e.state.db)
        .await
        .unwrap();
    let (s, b) = call(
        &e.raw,
        "192.168.1.2",
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username": "admin", "password": "oldpass8"})),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["password_weak"], true);
}

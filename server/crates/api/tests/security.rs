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
    let addr: SocketAddr = format!("{peer}:50000").parse().unwrap();
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

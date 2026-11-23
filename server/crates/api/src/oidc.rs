//! "Sign in with …" through OpenID Connect (#149): authorization code + PKCE, then Reel's normal
//! session. The browser never receives a provider token, and the login is bound to the browser that
//! started it (a secret kept in `sessionStorage`), so a link can't sign someone into another account.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use reel_auth::{
    AuthError, Device, NewOidcProvider, OidcProvider, OidcProviderPatch, VerifiedIdentity,
    oidc::{IdTokenCheck, Jwks, TokenError, verify_id_token},
    signing::now_unix,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, ClientIp, LoginResponse, Peer, login_response, on_admin_network},
    error::{ApiError, ApiResult, ErrorBody},
    mfa::MfaChallengeDto,
};

const FLOW_TTL: Duration = Duration::from_secs(10 * 60);
const TICKET_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING: usize = 2_000;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const METADATA_TTL: Duration = Duration::from_secs(3600);
/// A token with an unknown key refetches the key set at most this often.
const METADATA_MIN_REFRESH: Duration = Duration::from_secs(60);

const GOOGLE_ISSUER: &str = "https://accounts.google.com";
/// Microsoft's fixed tenant for personal accounts: work and school accounts have other issuers.
const MICROSOFT_CONSUMERS_ISSUER: &str =
    "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0";
const MICROSOFT_CONSUMERS_DISCOVERY: &str =
    "https://login.microsoftonline.com/consumers/v2.0/.well-known/openid-configuration";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Login,
    Invite,
    Link(i64),
}

struct Flow {
    provider_id: i64,
    purpose: Purpose,
    verifier: String,
    nonce: String,
    binding_hash: String,
    dev: Device,
    expires: Instant,
}

struct Ticket {
    identity: VerifiedIdentity,
    purpose: Purpose,
    binding_hash: String,
    dev: Device,
    expires: Instant,
}

#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Clone)]
struct Metadata {
    discovery: Discovery,
    jwks: Jwks,
    fetched: Instant,
}

/// Logins in progress and cached provider metadata. A restart just means signing in again.
#[derive(Default)]
pub struct OidcRuntime {
    flows: Mutex<HashMap<String, Flow>>,
    tickets: Mutex<HashMap<String, Ticket>>,
    metadata: tokio::sync::Mutex<HashMap<i64, Metadata>>,
    http: OnceLock<reqwest::Client>,
}

impl OidcRuntime {
    fn client(&self) -> &reqwest::Client {
        self.http.get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default()
        })
    }

    /// Forget cached endpoints and keys after a provider changes.
    async fn forget(&self, provider_id: i64) {
        self.metadata.lock().await.remove(&provider_id);
    }

    fn put_flow(&self, state: &str, flow: Flow) -> ApiResult<()> {
        let mut m = self.flows.lock().unwrap();
        if m.len() >= MAX_PENDING {
            let now = Instant::now();
            m.retain(|_, f| f.expires > now);
            if m.len() >= MAX_PENDING {
                return Err(ApiError::Locked(Duration::from_secs(10)));
            }
        }
        m.insert(digest(state), flow);
        Ok(())
    }

    /// Single use: a replayed `state` finds nothing.
    fn take_flow(&self, state: &str) -> Option<Flow> {
        self.flows
            .lock()
            .unwrap()
            .remove(&digest(state))
            .filter(|f| f.expires > Instant::now())
    }

    fn put_ticket(&self, ticket: Ticket) -> Option<String> {
        let id = random_token();
        let mut m = self.tickets.lock().unwrap();
        if m.len() >= MAX_PENDING {
            let now = Instant::now();
            m.retain(|_, t| t.expires > now);
            if m.len() >= MAX_PENDING {
                return None;
            }
        }
        m.insert(digest(&id), ticket);
        Some(id)
    }

    /// Take a ticket if the browser and purpose match. A mismatch doesn't burn it: whoever holds a
    /// leaked ticket without the browser's secret can't destroy the real login either.
    fn take_ticket(
        &self,
        id: &str,
        binding: &str,
        purpose: impl Fn(Purpose) -> bool,
    ) -> Option<Ticket> {
        let key = digest(id);
        let mut m = self.tickets.lock().unwrap();
        let t = m.get(&key)?;
        if t.expires <= Instant::now() {
            m.remove(&key);
            return None;
        }
        if t.binding_hash != digest(binding) || !purpose(t.purpose) {
            return None;
        }
        m.remove(&key)
    }

    /// Read an invite ticket without using it up: a taken username shouldn't cost the provider login.
    fn peek_invite(&self, id: &str, binding: &str) -> Option<VerifiedIdentity> {
        let m = self.tickets.lock().unwrap();
        m.get(&digest(id))
            .filter(|t| {
                t.expires > Instant::now()
                    && t.purpose == Purpose::Invite
                    && t.binding_hash == digest(binding)
            })
            .map(|t| t.identity.clone())
    }

    fn drop_ticket(&self, id: &str) {
        self.tickets.lock().unwrap().remove(&digest(id));
    }
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn digest(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}

/// Provider endpoints must be HTTPS. Plain HTTP is allowed only to this machine.
fn endpoint_ok(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| match u.scheme() {
        "https" => u.host().is_some(),
        "http" => matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    })
}

async fn read_capped(mut res: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(c) = res.chunk().await.map_err(|e| e.to_string())? {
        if out.len() + c.len() > MAX_RESPONSE_BYTES {
            return Err("response too large".into());
        }
        out.extend_from_slice(&c);
    }
    Ok(out)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    if !endpoint_ok(url) {
        return Err("endpoint must be https".into());
    }
    let res = client
        .get(url)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status().as_u16()));
    }
    serde_json::from_slice(&read_capped(res).await?).map_err(|e| e.to_string())
}

async fn metadata(st: &AppState, p: &OidcProvider, refresh: bool) -> Result<Metadata, String> {
    let mut cache = st.oidc.metadata.lock().await;
    if let Some(m) = cache.get(&p.id) {
        let age = m.fetched.elapsed();
        if age < METADATA_MIN_REFRESH || (!refresh && age < METADATA_TTL) {
            return Ok(m.clone());
        }
    }
    let client = st.oidc.client();
    let discovery: Discovery = get_json(client, &p.discovery_url).await?;
    if discovery.issuer != p.issuer {
        return Err("discovery document names a different issuer".into());
    }
    if !endpoint_ok(&discovery.authorization_endpoint) || !endpoint_ok(&discovery.token_endpoint) {
        return Err("provider endpoints must be https".into());
    }
    let jwks: Jwks = get_json(client, &discovery.jwks_uri).await?;
    let m = Metadata {
        discovery,
        jwks,
        fetched: Instant::now(),
    };
    cache.insert(p.id, m.clone());
    Ok(m)
}

fn public_base(st: &AppState) -> ApiResult<String> {
    st.config
        .public_url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            ApiError::Status(
                StatusCode::CONFLICT,
                "provider sign-in needs REEL_PUBLIC_URL to be set".into(),
            )
        })
}

fn redirect_uri(base: &str) -> String {
    format!("{base}/api/v1/auth/oidc/callback")
}

async fn begin(
    st: &AppState,
    p: &OidcProvider,
    purpose: Purpose,
    binding_hash: String,
    dev: Device,
) -> ApiResult<String> {
    if binding_hash.len() != 43 {
        return Err(ApiError::BadRequest("invalid browser binding".into()));
    }
    let base = public_base(st)?;
    let meta = metadata(st, p, false).await.map_err(|e| {
        tracing::warn!(provider = %p.slug, "OIDC discovery failed: {e}");
        ApiError::BadGateway("couldn't reach the sign-in provider".into())
    })?;
    let (state, nonce, verifier) = (random_token(), random_token(), random_token());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let url = reqwest::Url::parse_with_params(
        &meta.discovery.authorization_endpoint,
        [
            ("response_type", "code"),
            ("client_id", p.client_id.as_str()),
            ("redirect_uri", redirect_uri(&base).as_str()),
            ("scope", "openid email"),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ],
    )
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    st.oidc.put_flow(
        &state,
        Flow {
            provider_id: p.id,
            purpose,
            verifier,
            nonce,
            binding_hash,
            dev,
            expires: Instant::now() + FLOW_TTL,
        },
    )?;
    Ok(url.into())
}

// ───────────────────────── public: start and return ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct OidcProviderPublicDto {
    pub slug: String,
    pub name: String,
}

/// Sign-in providers to offer on the login page.
#[utoipa::path(get, path = "/auth/oidc/providers", security(()), responses((status = 200, body = Vec<OidcProviderPublicDto>)))]
pub async fn public_providers(
    State(st): State<AppState>,
) -> ApiResult<Json<Vec<OidcProviderPublicDto>>> {
    if st.config.public_url.as_deref().is_none_or(str::is_empty) {
        return Ok(Json(vec![]));
    }
    Ok(Json(
        st.auth
            .list_oidc_providers()
            .await?
            .into_iter()
            .filter(|p| p.enabled && p.secret_set)
            .map(|p| OidcProviderPublicDto {
                slug: p.slug,
                name: p.name,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OidcStartBody {
    /// base64url SHA-256 of a random secret the browser keeps until it returns.
    pub binding_hash: String,
    /// `login` (default) or `invite`.
    pub purpose: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OidcStartDto {
    /// Send the browser here.
    pub authorization_url: String,
}

/// Start signing in with a provider.
#[utoipa::path(
    post,
    path = "/auth/oidc/{slug}/start",
    security(()),
    params(("slug" = String, Path, description = "Provider")),
    request_body = OidcStartBody,
    responses((status = 200, body = OidcStartDto), (status = 404, body = ErrorBody), (status = 429, body = ErrorBody), (status = 502, body = ErrorBody))
)]
pub async fn start(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(slug): Path<String>,
    Json(b): Json<OidcStartBody>,
) -> ApiResult<Json<OidcStartDto>> {
    let key = format!("oidcstart:{}", reel_auth::ip_bucket(ip.as_deref()));
    st.auth
        .limiter()
        .reserve(&key, 30)
        .map_err(ApiError::Locked)?;
    let purpose = match b.purpose.as_deref() {
        None | Some("login") => Purpose::Login,
        Some("invite") => Purpose::Invite,
        _ => return Err(ApiError::BadRequest("unknown purpose".into())),
    };
    let p = st.auth.oidc_provider(&slug).await?;
    let authorization_url = begin(
        &st,
        &p,
        purpose,
        b.binding_hash,
        Device {
            device_id: b.device_id.map(|v| v.chars().take(128).collect()),
            device_name: b.device_name.map(|v| v.chars().take(64).collect()),
            client: b.client.map(|v| v.chars().take(32).collect()),
            ip,
        },
    )
    .await?;
    Ok(Json(OidcStartDto { authorization_url }))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Exchange the code and verify the ID token; `Err` is a short reason for the log and audit trail.
async fn verify_callback(
    st: &AppState,
    p: &OidcProvider,
    flow: &Flow,
    code: &str,
    base: &str,
) -> Result<VerifiedIdentity, String> {
    let meta = metadata(st, p, false).await?;
    let secret = st
        .auth
        .oidc_client_secret(p.id)
        .await
        .map_err(|_| "no client secret".to_string())?;
    let form = form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", &redirect_uri(base))
        .append_pair("client_id", &p.client_id)
        .append_pair("client_secret", &secret)
        .append_pair("code_verifier", &flow.verifier)
        .finish();
    let res = st
        .oidc
        .client()
        .post(&meta.discovery.token_endpoint)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .body(form)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !res.status().is_success() {
        return Err(format!("token endpoint: HTTP {}", res.status().as_u16()));
    }
    let body: Value =
        serde_json::from_slice(&read_capped(res).await?).map_err(|_| "bad token response")?;
    let id_token = body["id_token"].as_str().ok_or("no id_token")?;
    let check = IdTokenCheck {
        issuer: &p.issuer,
        client_id: &p.client_id,
        nonce: &flow.nonce,
        now: now_unix(),
    };
    let claims = match verify_id_token(id_token, &meta.jwks, &check) {
        Err(TokenError::UnknownKey) => {
            let fresh = metadata(st, p, true).await?;
            verify_id_token(id_token, &fresh.jwks, &check)
        }
        r => r,
    }
    .map_err(|e| e.to_string())?;
    Ok(VerifiedIdentity {
        provider_id: p.id,
        issuer: p.issuer.clone(),
        subject: claims.subject,
        label: claims.email,
    })
}

/// Back in the browser, without a token in any URL that gets logged: the ticket is in the fragment.
fn back_to_app(base: &str, fragment: &str) -> Response {
    let mut r = Redirect::to(&format!("{base}/login/oidc#{fragment}")).into_response();
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}

/// Where the provider sends the browser back.
#[utoipa::path(
    get,
    path = "/auth/oidc/callback",
    security(()),
    params(CallbackQuery),
    responses((status = 303, description = "Back to the web app"))
)]
pub async fn callback(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Query(q): Query<CallbackQuery>,
) -> ApiResult<Response> {
    let base = public_base(&st)?;
    let Some(flow) = q.state.as_deref().and_then(|s| st.oidc.take_flow(s)) else {
        return Ok(back_to_app(&base, "error=expired"));
    };
    if q.error.is_some() {
        return Ok(back_to_app(&base, "error=denied"));
    }
    let Some(code) = q.code.as_deref().filter(|c| c.len() <= 4096) else {
        return Ok(back_to_app(&base, "error=failed"));
    };
    let p = st.auth.oidc_provider_by_id(flow.provider_id).await?;
    let identity = match verify_callback(&st, &p, &flow, code, &base).await {
        Ok(i) => i,
        Err(reason) => {
            tracing::warn!(provider = %p.slug, "OIDC login failed: {reason}");
            st.auth
                .audit(
                    None,
                    None,
                    "login_oidc_failure",
                    Some(&p.slug),
                    json!({"reason": reason.chars().take(120).collect::<String>()}),
                    ip.as_deref(),
                )
                .await;
            return Ok(back_to_app(&base, "error=failed"));
        }
    };
    let ticket = st.oidc.put_ticket(Ticket {
        identity,
        purpose: flow.purpose,
        binding_hash: flow.binding_hash,
        dev: flow.dev,
        expires: Instant::now() + TICKET_TTL,
    });
    Ok(match ticket {
        Some(t) => back_to_app(&base, &format!("ticket={t}")),
        None => back_to_app(&base, "error=failed"),
    })
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OidcExchangeBody {
    pub ticket: String,
    /// The secret whose hash was sent to `/start`.
    pub binding: String,
}

/// Finish a provider sign-in. An identity that isn't linked to a Reel account is refused.
#[utoipa::path(
    post,
    path = "/auth/oidc/exchange",
    security(()),
    request_body = OidcExchangeBody,
    responses(
        (status = 200, body = LoginResponse),
        (status = 202, description = "Second factor needed", body = MfaChallengeDto),
        (status = 401, body = ErrorBody),
        (status = 429, body = ErrorBody)
    )
)]
pub async fn exchange(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<OidcExchangeBody>,
) -> ApiResult<Response> {
    let ip_key = format!("ip:{}", reel_auth::ip_bucket(ip.as_deref()));
    let ip_max = st.auth.limiter().config().ip_max_failures;
    st.auth
        .limiter()
        .reserve(&ip_key, ip_max)
        .map_err(ApiError::Locked)?;
    let refused = || ApiError::Unauthorized("that sign-in isn't linked to a Reel account".into());
    let Some(t) = st
        .oidc
        .take_ticket(&b.ticket, &b.binding, |p| p == Purpose::Login)
    else {
        return Err(ApiError::Unauthorized(
            "this sign-in expired; try again".into(),
        ));
    };
    let Some(user_id) = st.auth.identity_user(&t.identity).await? else {
        st.auth
            .audit(
                None,
                None,
                "login_oidc_unlinked",
                Some(&t.identity.provider_id.to_string()),
                Value::Null,
                ip.as_deref(),
            )
            .await;
        return Err(refused());
    };
    let out = match st.auth.external_login(user_id, &t.dev).await {
        Ok(o) => o,
        Err(AuthError::SecondFactorRequired(c)) => {
            st.auth.limiter().refund(&ip_key, ip_max);
            return Ok((StatusCode::ACCEPTED, Json(MfaChallengeDto::from(c))).into_response());
        }
        Err(AuthError::Unauthorized) => return Err(refused()),
        Err(e) => return Err(e.into()),
    };
    st.auth.limiter().refund(&ip_key, ip_max);
    Ok(Json(login_response(&st, out, peer, &headers).await?).into_response())
}

/// The verified identity behind an invite ticket, without using the ticket up.
pub(crate) fn invite_identity(
    st: &AppState,
    ticket: &str,
    binding: &str,
) -> ApiResult<VerifiedIdentity> {
    st.oidc.peek_invite(ticket, binding).ok_or_else(|| {
        ApiError::BadRequest("the provider sign-in expired; continue with it again".into())
    })
}

pub(crate) fn finish_invite_ticket(st: &AppState, ticket: &str) {
    st.oidc.drop_ticket(ticket);
}

// ───────────────────────── account: linked sign-ins ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct IdentityDto {
    pub id: i64,
    pub provider: String,
    /// The provider account's email, for recognizing it.
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Provider sign-ins linked to this account.
#[utoipa::path(get, path = "/me/identities", responses((status = 200, body = Vec<IdentityDto>)))]
pub async fn my_identities(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<Vec<IdentityDto>>> {
    Ok(Json(
        st.auth
            .list_identities(u.id())
            .await?
            .into_iter()
            .map(|i| IdentityDto {
                id: i.id,
                provider: i.provider,
                label: i.label,
                created_at: i.created_at,
                last_used_at: i.last_used_at,
            })
            .collect(),
    ))
}

fn guard_identity_change(
    st: &AppState,
    u: &AuthUser,
    peer: Option<std::net::IpAddr>,
    headers: &HeaderMap,
) -> ApiResult<()> {
    if u.user.has_admin_role() && !on_admin_network(&st.config, peer, headers) {
        return Err(ApiError::Forbidden(
            "admin accounts change sign-in methods from the home network or Tailscale".into(),
        ));
    }
    Ok(())
}

fn wrong_password(e: AuthError) -> ApiError {
    match e {
        AuthError::InvalidCredentials => ApiError::BadRequest("that password isn't right".into()),
        e => e.into(),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkStartBody {
    pub password: String,
    pub binding_hash: String,
}

/// Start linking a provider sign-in to this account.
#[utoipa::path(
    post,
    path = "/me/identities/{slug}/start",
    params(("slug" = String, Path, description = "Provider")),
    request_body = LinkStartBody,
    responses((status = 200, body = OidcStartDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn link_start(
    State(st): State<AppState>,
    u: AuthUser,
    ClientIp(ip): ClientIp,
    Peer(peer): Peer,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(b): Json<LinkStartBody>,
) -> ApiResult<Json<OidcStartDto>> {
    guard_identity_change(&st, &u, peer, &headers)?;
    st.auth
        .confirm_password(&u.user, &b.password)
        .await
        .map_err(wrong_password)?;
    let p = st.auth.oidc_provider(&slug).await?;
    let dev = Device {
        ip,
        ..Default::default()
    };
    let authorization_url = begin(&st, &p, Purpose::Link(u.id()), b.binding_hash, dev).await?;
    Ok(Json(OidcStartDto { authorization_url }))
}

/// Finish linking: the provider login becomes a way into this account.
#[utoipa::path(
    post,
    path = "/me/identities/complete",
    request_body = OidcExchangeBody,
    responses((status = 204, description = "Linked"), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn link_complete(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<OidcExchangeBody>,
) -> ApiResult<StatusCode> {
    guard_identity_change(&st, &u, peer, &headers)?;
    let uid = u.id();
    let t = st
        .oidc
        .take_ticket(&b.ticket, &b.binding, |p| p == Purpose::Link(uid))
        .ok_or_else(|| ApiError::BadRequest("this link expired; try again".into()))?;
    st.auth.link_identity(&u.user, &t.identity).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UnlinkBody {
    pub password: String,
}

/// Remove a linked provider sign-in.
#[utoipa::path(
    post,
    path = "/me/identities/{id}/remove",
    params(("id" = i64, Path, description = "Identity id")),
    request_body = UnlinkBody,
    responses((status = 204, description = "Removed"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn unlink(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<UnlinkBody>,
) -> ApiResult<StatusCode> {
    guard_identity_change(&st, &u, peer, &headers)?;
    st.auth
        .unlink_identity(&u.user, &b.password, id)
        .await
        .map_err(wrong_password)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────────────────── admin: providers ─────────────────────────

/// A provider as admins see it; the client secret is never returned.
#[derive(Debug, Serialize, ToSchema)]
pub struct OidcProviderDto {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub issuer: String,
    pub client_id: String,
    pub secret_set: bool,
    pub enabled: bool,
    /// Register this redirect URI at the provider.
    pub redirect_uri: Option<String>,
}

fn provider_dto(st: &AppState, p: OidcProvider) -> OidcProviderDto {
    OidcProviderDto {
        redirect_uri: public_base(st).ok().map(|b| redirect_uri(&b)),
        id: p.id,
        slug: p.slug,
        name: p.name,
        issuer: p.issuer,
        client_id: p.client_id,
        secret_set: p.secret_set,
        enabled: p.enabled,
    }
}

/// Configured sign-in providers.
#[utoipa::path(get, path = "/admin/oidc/providers", responses((status = 200, body = Vec<OidcProviderDto>)))]
pub async fn admin_list(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<OidcProviderDto>>> {
    let list = st.auth.list_oidc_providers().await?;
    Ok(Json(
        list.into_iter().map(|p| provider_dto(&st, p)).collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOidcProviderBody {
    /// `google`, `microsoft` (personal accounts) or `custom`.
    pub preset: String,
    /// Display name; required for `custom`.
    pub name: Option<String>,
    /// Issuer URL; required for `custom`.
    pub issuer: Option<String>,
    pub client_id: String,
    pub client_secret: String,
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

/// Add a provider.
#[utoipa::path(
    post,
    path = "/admin/oidc/providers",
    request_body = CreateOidcProviderBody,
    responses((status = 201, body = OidcProviderDto), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn admin_create(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<CreateOidcProviderBody>,
) -> ApiResult<(StatusCode, Json<OidcProviderDto>)> {
    let (slug, name, issuer, discovery_url) = match b.preset.as_str() {
        "google" => (
            "google".to_string(),
            "Google".to_string(),
            GOOGLE_ISSUER.to_string(),
            format!("{GOOGLE_ISSUER}/.well-known/openid-configuration"),
        ),
        "microsoft" => (
            "microsoft".to_string(),
            "Microsoft".to_string(),
            MICROSOFT_CONSUMERS_ISSUER.to_string(),
            MICROSOFT_CONSUMERS_DISCOVERY.to_string(),
        ),
        "custom" => {
            let name = b
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .ok_or_else(|| ApiError::BadRequest("a custom provider needs a name".into()))?;
            let issuer = b.issuer.as_deref().map(str::trim).unwrap_or_default();
            if !endpoint_ok(issuer) {
                return Err(ApiError::BadRequest(
                    "the issuer must be an https URL".into(),
                ));
            }
            let slug = slugify(&name);
            if slug.is_empty() {
                return Err(ApiError::BadRequest("that name can't be used".into()));
            }
            let discovery = format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            );
            (slug, name, issuer.to_string(), discovery)
        }
        _ => return Err(ApiError::BadRequest("unknown preset".into())),
    };
    let p = st
        .auth
        .create_oidc_provider(NewOidcProvider {
            slug,
            name,
            issuer,
            discovery_url,
            client_id: b.client_id,
            client_secret: b.client_secret,
        })
        .await?;
    st.audit(
        &a.user,
        "oidc_provider_create",
        Some(p.id.to_string()),
        json!({"slug": p.slug}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(provider_dto(&st, p))))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchOidcProviderBody {
    pub name: Option<String>,
    pub client_id: Option<String>,
    /// Omit or leave empty to keep the current secret.
    pub client_secret: Option<String>,
    pub enabled: Option<bool>,
}

/// Change a provider.
#[utoipa::path(
    patch,
    path = "/admin/oidc/providers/{id}",
    params(("id" = i64, Path, description = "Provider id")),
    request_body = PatchOidcProviderBody,
    responses((status = 200, body = OidcProviderDto), (status = 404, body = ErrorBody))
)]
pub async fn admin_patch(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<PatchOidcProviderBody>,
) -> ApiResult<Json<OidcProviderDto>> {
    let detail = json!({
        "name": b.name, "enabled": b.enabled, "client_id": b.client_id.is_some(),
        "client_secret": b.client_secret.as_deref().is_some_and(|s| !s.is_empty()),
    });
    let p = st
        .auth
        .update_oidc_provider(
            id,
            OidcProviderPatch {
                name: b.name,
                client_id: b.client_id,
                client_secret: b.client_secret,
                enabled: b.enabled,
            },
        )
        .await?;
    st.oidc.forget(id).await;
    st.audit(
        &a.user,
        "oidc_provider_update",
        Some(id.to_string()),
        detail,
    )
    .await;
    Ok(Json(provider_dto(&st, p)))
}

/// Remove a provider and the sign-ins linked through it.
#[utoipa::path(
    delete,
    path = "/admin/oidc/providers/{id}",
    params(("id" = i64, Path, description = "Provider id")),
    responses((status = 204, description = "Removed"), (status = 404, body = ErrorBody))
)]
pub async fn admin_delete(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.delete_oidc_provider(id).await?;
    st.oidc.forget(id).await;
    st.audit(
        &a.user,
        "oidc_provider_delete",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_endpoints_must_be_https_except_on_this_machine() {
        assert!(endpoint_ok("https://accounts.google.com/x"));
        assert!(endpoint_ok("http://127.0.0.1:9000/x"));
        assert!(endpoint_ok("http://localhost:9000/x"));
        for bad in [
            "http://accounts.google.com/x",
            "http://10.0.0.1/x",
            "ftp://a/x",
            "file:///etc/passwd",
            "nope",
        ] {
            assert!(!endpoint_ok(bad), "{bad}");
        }
    }

    #[test]
    fn slugs_are_url_safe() {
        assert_eq!(slugify("Pocket ID (home)"), "pocket-id-home");
        assert_eq!(slugify("  !!  "), "");
    }
}

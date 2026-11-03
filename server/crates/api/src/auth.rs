//! Authentication, authorization, signed URLs and the account/invite/audit routes (SPEC §8).

use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
};
use reel_auth::{Device, Limits, NewUser, Role, User, UserPatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    error::{ApiError, ApiResult, ErrorBody},
};

// ───────────────────────── extractors ─────────────────────────

/// An authenticated user (`Authorization: Bearer <token>`).
pub struct AuthUser {
    pub user: User,
    /// `sessions_auth.id` of the presented token.
    pub session_id: i64,
}

impl AuthUser {
    pub fn id(&self) -> i64 {
        self.user.id
    }
    pub fn is_admin(&self) -> bool {
        self.user.is_admin()
    }
}

/// An authenticated admin; 403 for other users.
pub struct AdminUser(pub AuthUser);

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|t| !t.is_empty())
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let token = bearer(&parts.headers)
            .ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
        let (user, session) = st.auth.authenticate(token).await?;
        Ok(AuthUser {
            user,
            session_id: session.id,
        })
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let u = AuthUser::from_request_parts(parts, st).await?;
        if !u.is_admin() {
            return Err(ApiError::Forbidden("admin access required".into()));
        }
        Ok(AdminUser(u))
    }
}

/// Client IP for rate limiting / audit.
///
/// `X-Forwarded-For` / `X-Real-IP` are believed only when the socket peer is a trusted proxy
/// (`trusted_proxies` CIDRs; the deprecated `trust_proxy_headers` means "private-range peers").
/// The client is then the right-most XFF entry that is not itself a trusted proxy, so a client
/// cannot pick its own address by prepending entries.
pub struct ClientIp(pub Option<String>);

/// Resolve the client address from the socket peer and the proxy headers.
pub fn resolve_client_ip(
    cfg: &reel_core::Config,
    peer: Option<std::net::IpAddr>,
    headers: &HeaderMap,
) -> Option<std::net::IpAddr> {
    use crate::netutil::{is_private, normalize, parse_cidrs};
    let cidrs = parse_cidrs(&cfg.trusted_proxies);
    let alias = cfg.trust_proxy_headers;
    let trusted =
        |ip: std::net::IpAddr| cidrs.iter().any(|c| c.contains(ip)) || (alias && is_private(ip));
    // no socket info (in-process tests): only the deprecated blanket switch believes headers
    let peer_trusted = match peer {
        Some(p) => trusted(p),
        None => alias,
    };
    if !peer_trusted {
        return peer.map(normalize);
    }
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let mut chain: Vec<std::net::IpAddr> = h("x-forwarded-for")
        .map(|v| {
            v.split(',')
                .filter_map(|p| p.trim().parse::<std::net::IpAddr>().ok())
                .collect()
        })
        .unwrap_or_default();
    if chain.is_empty() {
        if let Some(r) = h("x-real-ip").and_then(|v| v.trim().parse().ok()) {
            chain.push(r);
        }
    }
    chain.extend(peer);
    // right-most entry that is not a trusted proxy
    chain
        .iter()
        .rev()
        .find(|ip| !trusted(**ip))
        .or_else(|| chain.first())
        .copied()
        .map(normalize)
}

/// The socket peer (`None` without connection info, i.e. in-process tests).
pub struct Peer(pub Option<std::net::IpAddr>);

impl FromRequestParts<AppState> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0.ip()),
        ))
    }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip());
        Ok(ClientIp(
            resolve_client_ip(&st.config, peer, &parts.headers).map(|i| i.to_string()),
        ))
    }
}

/// Signature query parameters of a signed media URL.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct SignedQuery {
    /// Expiry (unix seconds).
    pub exp: Option<i64>,
    /// User the URL was signed for.
    pub u: Option<i64>,
    /// HMAC-SHA256 signature (base64url).
    pub sig: Option<String>,
}

impl AppState {
    /// URL signer for `user_id`.
    pub fn signer(&self, user_id: i64) -> reel_auth::UrlSigner {
        self.auth.signer(user_id)
    }

    /// TTL (seconds) of signed stream/direct/subtitle URLs.
    pub fn media_ttl(&self) -> i64 {
        self.config.signed_url_ttl_secs as i64
    }

    /// Resolve the caller of a media endpoint: bearer token, or a valid signature for `scope`.
    pub async fn media_user(
        &self,
        headers: &HeaderMap,
        q: &SignedQuery,
        scope: &str,
    ) -> ApiResult<User> {
        if let Some(t) = bearer(headers) {
            return Ok(self.auth.authenticate(t).await?.0);
        }
        match (q.exp, q.u, q.sig.as_deref()) {
            (Some(exp), Some(u), Some(sig)) => {
                Ok(self.auth.verify_signature(scope, u, exp, sig).await?)
            }
            _ => Err(ApiError::Unauthorized(
                "authentication required (token or signed URL)".into(),
            )),
        }
    }

    pub async fn audit(&self, actor: &User, action: &str, target: Option<String>, detail: Value) {
        self.auth
            .audit(
                Some(actor.id),
                Some(&actor.username),
                action,
                target.as_deref(),
                detail,
                None,
            )
            .await;
    }

    /// Library ids visible to `user` (`None` = all).
    pub async fn visible(&self, user: &User) -> ApiResult<Option<Vec<i64>>> {
        Ok(self.auth.visible_libraries(user).await?)
    }

    /// 404 unless `user` may see `library_id`.
    pub async fn require_library(&self, user: &User, library_id: i64) -> ApiResult<()> {
        if user.is_admin() {
            return Ok(());
        }
        let ok: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM library_access WHERE user_id = ? AND library_id = ?")
                .bind(user.id)
                .bind(library_id)
                .fetch_optional(&self.db)
                .await?;
        ok.map(|_| ()).ok_or(ApiError::NotFound)
    }

    /// Library of an item; 404 if missing or not visible.
    pub async fn require_item(&self, user: &User, item_id: i64) -> ApiResult<i64> {
        let lib: i64 = sqlx::query_scalar("SELECT library_id FROM media_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&self.db)
            .await?
            .ok_or(ApiError::NotFound)?;
        self.require_library(user, lib).await?;
        Ok(lib)
    }

    /// Item id of a media file; 404 if missing or not visible.
    pub async fn require_file(&self, user: &User, file_id: i64) -> ApiResult<i64> {
        let item: i64 = sqlx::query_scalar("SELECT item_id FROM media_files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(&self.db)
            .await?
            .ok_or(ApiError::NotFound)?;
        self.require_item(user, item).await?;
        Ok(item)
    }
}

/// JSON array of visible library ids for `json_each` filters (`None` = no filter).
pub fn vis_json(v: &Option<Vec<i64>>) -> Option<String> {
    v.as_ref()
        .map(|ids| serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
}

/// SQL fragment filtering `col` by visibility; binds twice (see [`bind_vis`]).
pub fn vis_clause(col: &str) -> String {
    format!("(? IS NULL OR {col} IN (SELECT value FROM json_each(?)))")
}

// ───────────────────────── DTOs ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleDto {
    Admin,
    User,
}

impl From<Role> for RoleDto {
    fn from(r: Role) -> Self {
        match r {
            Role::Admin => Self::Admin,
            Role::User => Self::User,
        }
    }
}

impl From<RoleDto> for Role {
    fn from(r: RoleDto) -> Self {
        match r {
            RoleDto::Admin => Self::Admin,
            RoleDto::User => Self::User,
        }
    }
}

/// Per-user limits; `null` = unlimited.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct LimitsDto {
    /// Concurrent playback sessions.
    pub max_streams: Option<i64>,
    /// Concurrent transcoding sessions.
    pub max_transcodes: Option<i64>,
    /// Bitrate cap in kbit/s (forces transcoding to a lower bitrate).
    pub max_bitrate_kbps: Option<i64>,
    pub request_quota_movies: Option<i64>,
    pub request_quota_tv: Option<i64>,
    pub quota_window_days: i64,
    /// Requests are approved and sent to Radarr/Sonarr immediately.
    pub auto_approve: bool,
}

impl Default for LimitsDto {
    fn default() -> Self {
        Limits::default().into()
    }
}

impl From<Limits> for LimitsDto {
    fn from(l: Limits) -> Self {
        Self {
            max_streams: l.max_streams,
            max_transcodes: l.max_transcodes,
            max_bitrate_kbps: l.max_bitrate_kbps,
            request_quota_movies: l.request_quota_movies,
            request_quota_tv: l.request_quota_tv,
            quota_window_days: l.quota_window_days,
            auto_approve: l.auto_approve,
        }
    }
}

impl From<LimitsDto> for Limits {
    fn from(l: LimitsDto) -> Self {
        Self {
            max_streams: l.max_streams,
            max_transcodes: l.max_transcodes,
            max_bitrate_kbps: l.max_bitrate_kbps,
            request_quota_movies: l.request_quota_movies,
            request_quota_tv: l.request_quota_tv,
            quota_window_days: l.quota_window_days,
            auto_approve: l.auto_approve,
        }
    }
}

/// What a user may use. Admins can do everything regardless.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct PermissionsDto {
    /// Live TV (IPTV channels, guide).
    pub iptv_access: bool,
    /// Provider VOD catalog.
    pub vod_access: bool,
    /// Request movies / shows (and see requests).
    pub can_request: bool,
    /// Play on TVs (DLNA devices).
    pub can_cast: bool,
}

impl From<reel_auth::Permissions> for PermissionsDto {
    fn from(p: reel_auth::Permissions) -> Self {
        Self {
            iptv_access: p.iptv_access,
            vod_access: p.vod_access,
            can_request: p.can_request,
            can_cast: p.can_cast,
        }
    }
}

impl From<PermissionsDto> for reel_auth::Permissions {
    fn from(p: PermissionsDto) -> Self {
        Self {
            iptv_access: p.iptv_access,
            vod_access: p.vod_access,
            can_request: p.can_request,
            can_cast: p.can_cast,
        }
    }
}

fn effective(u: &User) -> PermissionsDto {
    if u.is_admin() {
        reel_auth::Permissions::all().into()
    } else {
        u.perms.into()
    }
}

/// The signed-in user.
#[derive(Debug, Serialize, ToSchema)]
pub struct Me {
    pub id: i64,
    pub username: String,
    pub role: RoleDto,
    pub limits: LimitsDto,
    /// Effective permissions (all true for admins).
    pub permissions: PermissionsDto,
    /// Visible library ids; absent = all libraries (admin).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_ids: Option<Vec<i64>>,
}

async fn me_dto(st: &AppState, u: &User) -> ApiResult<Me> {
    Ok(Me {
        id: u.id,
        username: u.username.clone(),
        role: u.role.into(),
        limits: u.limits.clone().into(),
        permissions: effective(u),
        library_ids: st.visible(u).await?,
    })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    /// Opaque bearer token: send as `Authorization: Bearer <token>`
    /// (or `?token=` on the `/events` WebSocket).
    pub token: String,
    /// Session expiry (unix seconds); extended on use.
    pub expires_at: i64,
    pub user: Me,
}

async fn login_response(st: &AppState, o: reel_auth::LoginOutcome) -> ApiResult<LoginResponse> {
    Ok(LoginResponse {
        user: me_dto(st, &o.user).await?,
        token: o.token,
        expires_at: o.expires_at,
    })
}

// ───────────────────────── setup ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatus {
    /// True until the first admin account has been created.
    pub needs_setup: bool,
}

/// Whether first-run setup is still open.
#[utoipa::path(get, path = "/setup", security(()), responses((status = 200, body = SetupStatus)))]
pub async fn setup_status(State(st): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    Ok(Json(SetupStatus {
        needs_setup: st.auth.needs_setup().await?,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetupBody {
    pub username: String,
    /// At least 8 characters.
    pub password: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
    /// One-time token printed in the server log at first boot (needed when not on the LAN).
    pub setup_token: Option<String>,
}

/// Create the first admin account (only while `needs_setup`); returns a signed-in session.
#[utoipa::path(
    post,
    path = "/setup",
    security(()),
    request_body = SetupBody,
    responses(
        (status = 201, body = LoginResponse),
        (status = 400, body = ErrorBody),
        (status = 409, description = "Setup already completed", body = ErrorBody)
    )
)]
pub async fn setup(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<SetupBody>,
) -> ApiResult<(StatusCode, Json<LoginResponse>)> {
    // cheapest refusals first: nothing to claim, or too many tries from this address
    if !st.auth.needs_setup().await? {
        return Err(reel_auth::AuthError::Conflict("setup was already completed".into()).into());
    }
    let bucket = format!("setup:{}", reel_auth::ip_bucket(ip.as_deref()));
    st.auth
        .limiter()
        .reserve(&bucket, 10)
        .map_err(ApiError::Locked)?;
    // Only the LAN may claim a fresh server, or whoever has the one-time token from the server log.
    let presented = headers
        .get("x-setup-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| b.setup_token.clone());
    let token_ok = st.auth.setup_token_ok(presented.as_deref());
    let client = ip
        .as_deref()
        .and_then(|i| i.parse::<std::net::IpAddr>().ok());
    let forwarded = ["x-forwarded-for", "x-real-ip", "forwarded"]
        .iter()
        .any(|h| headers.contains_key(*h));
    // proxy headers that were not believed: the peer is a proxy, not the client
    let unbelieved = forwarded && peer.map(crate::netutil::normalize) == client;
    let on_lan = match client {
        Some(c) => crate::netutil::is_private(c) && !unbelieved,
        None => peer.is_none(), // in-process (tests)
    };
    if !(token_ok || on_lan) {
        return Err(ApiError::Forbidden(
            "first-run setup is only accepted from the local network, or with the one-time setup \
             token printed in the server log (send it as `setup_token` or `X-Setup-Token`)"
                .into(),
        ));
    }
    st.auth.setup(&b.username, &b.password).await?;
    st.auth.limiter().clear(&bucket);
    let out = st
        .auth
        .login(
            &b.username,
            &b.password,
            Device {
                device_id: b.device_id,
                device_name: b.device_name,
                client: b.client,
                ip,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(login_response(&st, out).await?)))
}

// ───────────────────────── login / logout / me ─────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    /// Stable per-install id; logging in again from the same device replaces its session.
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    /// e.g. `web`, `webos`, `tizen`, `android`.
    pub client: Option<String>,
}

/// Sign in. Repeated failures lock the username and the client IP with exponential backoff
/// (429 + `Retry-After`).
#[utoipa::path(
    post,
    path = "/auth/login",
    security(()),
    request_body = LoginBody,
    responses(
        (status = 200, body = LoginResponse),
        (status = 401, body = ErrorBody),
        (status = 429, description = "Locked out", body = ErrorBody)
    )
)]
pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(b): Json<LoginBody>,
) -> ApiResult<Json<LoginResponse>> {
    let out = st
        .auth
        .login(
            &b.username,
            &b.password,
            Device {
                device_id: b.device_id,
                device_name: b.device_name,
                client: b.client,
                ip,
            },
        )
        .await?;
    Ok(Json(login_response(&st, out).await?))
}

/// Revoke the current token.
#[utoipa::path(post, path = "/auth/logout", responses((status = 204, description = "Signed out")))]
pub async fn logout(State(st): State<AppState>, u: AuthUser) -> ApiResult<StatusCode> {
    st.auth.logout(u.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The current user.
#[utoipa::path(get, path = "/me", responses((status = 200, body = Me), (status = 401, body = ErrorBody)))]
pub async fn me(State(st): State<AppState>, u: AuthUser) -> ApiResult<Json<Me>> {
    Ok(Json(me_dto(&st, &u.user).await?))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionDto {
    pub id: i64,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
    pub ip: Option<String>,
    pub last_seen: i64,
    pub expires_at: Option<i64>,
    /// This is the session making the request.
    pub current: bool,
}

/// Devices signed in as the current user.
#[utoipa::path(get, path = "/me/sessions", responses((status = 200, body = Vec<SessionDto>)))]
pub async fn my_sessions(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<Vec<SessionDto>>> {
    let list = st.auth.list_sessions(u.id()).await?;
    Ok(Json(
        list.into_iter()
            .map(|s| SessionDto {
                current: s.id == u.session_id,
                id: s.id,
                device_id: s.device_id,
                device_name: s.device_name,
                client: s.client,
                ip: s.ip,
                last_seen: s.last_seen,
                expires_at: s.expires_at,
            })
            .collect(),
    ))
}

/// Sign a device out.
#[utoipa::path(
    delete,
    path = "/me/sessions/{id}",
    params(("id" = i64, Path, description = "Auth session id")),
    responses((status = 204, description = "Revoked"), (status = 404, body = ErrorBody))
)]
pub async fn revoke_my_session(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.revoke_session(u.id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasswordBody {
    pub current_password: String,
    /// At least 8 characters.
    pub new_password: String,
}

/// Change the password; all other sessions are signed out.
#[utoipa::path(
    post,
    path = "/me/password",
    request_body = PasswordBody,
    responses((status = 204, description = "Changed"), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody))
)]
pub async fn change_password(
    State(st): State<AppState>,
    u: AuthUser,
    ClientIp(ip): ClientIp,
    Json(b): Json<PasswordBody>,
) -> ApiResult<StatusCode> {
    // A wrong current password counts against the user's lockout like a failed login.
    let key = format!("user:{}", u.user.username.to_lowercase());
    st.auth.limiter().check(&key).map_err(ApiError::Locked)?;
    match st
        .auth
        .change_password(
            u.id(),
            &b.current_password,
            &b.new_password,
            Some(u.session_id),
        )
        .await
    {
        Err(reel_auth::AuthError::InvalidCredentials) => {
            st.auth.limiter().fail_user(&key);
            Err(ApiError::Unauthorized(
                "current password is incorrect".into(),
            ))
        }
        r => {
            r?;
            st.auth
                .audit(
                    Some(u.id()),
                    Some(&u.user.username),
                    "password_change",
                    None,
                    Value::Null,
                    ip.as_deref(),
                )
                .await;
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

// ───────────────────────── invites (public side) ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct InviteInfoDto {
    pub valid: bool,
    pub expires_at: Option<i64>,
    /// Names of the libraries the invite grants.
    pub library_names: Vec<String>,
}

/// Check an invite code (for the landing page).
#[utoipa::path(
    get,
    path = "/auth/invite/{code}",
    security(()),
    params(("code" = String, Path, description = "Invite code")),
    responses((status = 200, body = InviteInfoDto))
)]
pub async fn invite_info(
    State(st): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<InviteInfoDto>> {
    let i = st.auth.invite_info(&code).await?;
    Ok(Json(InviteInfoDto {
        valid: i.valid,
        expires_at: i.expires_at,
        library_names: i.library_names,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AcceptInviteBody {
    pub username: String,
    /// At least 8 characters.
    pub password: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
}

/// Redeem a single-use invite: creates the account (with the invite's library access and
/// limits) and signs it in.
#[utoipa::path(
    post,
    path = "/auth/invite/{code}/accept",
    security(()),
    params(("code" = String, Path, description = "Invite code")),
    request_body = AcceptInviteBody,
    responses(
        (status = 201, body = LoginResponse),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown, used or expired invite", body = ErrorBody),
        (status = 409, description = "Username taken", body = ErrorBody)
    )
)]
pub async fn accept_invite(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
    Json(b): Json<AcceptInviteBody>,
) -> ApiResult<(StatusCode, Json<LoginResponse>)> {
    let out = st
        .auth
        .accept_invite(
            &code,
            &b.username,
            &b.password,
            Device {
                device_id: b.device_id,
                device_name: b.device_name,
                client: b.client,
                ip,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(login_response(&st, out).await?)))
}

// ───────────────────────── admin: users ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: i64,
    pub username: String,
    pub role: RoleDto,
    pub disabled: bool,
    pub created_at: i64,
    pub limits: LimitsDto,
    pub permissions: PermissionsDto,
    /// Libraries the user may access (ignored for admins, who see everything).
    pub library_ids: Vec<i64>,
}

async fn user_dto(st: &AppState, u: User) -> ApiResult<UserDto> {
    Ok(UserDto {
        library_ids: st.auth.library_ids(u.id).await?,
        id: u.id,
        username: u.username,
        role: u.role.into(),
        disabled: u.disabled,
        created_at: u.created_at,
        permissions: u.perms.into(),
        limits: u.limits.into(),
    })
}

/// All accounts.
#[utoipa::path(get, path = "/admin/users", responses((status = 200, body = Vec<UserDto>)))]
pub async fn list_users(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<UserDto>>> {
    let mut out = Vec::new();
    for u in st.auth.list_users().await? {
        out.push(user_dto(&st, u).await?);
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserBody {
    pub username: String,
    pub password: String,
    pub role: Option<RoleDto>,
    pub limits: Option<LimitsDto>,
    #[serde(default)]
    pub library_ids: Vec<i64>,
    /// Feature permissions (default: all on for accounts an admin creates directly).
    pub permissions: Option<PermissionsDto>,
}

/// Create an account directly (without an invite).
#[utoipa::path(
    post,
    path = "/admin/users",
    request_body = CreateUserBody,
    responses((status = 201, body = UserDto), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn create_user(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<CreateUserBody>,
) -> ApiResult<(StatusCode, Json<UserDto>)> {
    let u = st
        .auth
        .create_user(NewUser {
            username: b.username,
            password: b.password,
            role: b.role.map_or(Role::User, Into::into),
            limits: b.limits.map(Into::into).unwrap_or_default(),
            library_ids: b.library_ids,
        })
        .await?;
    let u = match b.permissions {
        Some(p) => {
            st.auth
                .update_user(
                    u.id,
                    UserPatch {
                        perms: Some(p.into()),
                        ..Default::default()
                    },
                )
                .await?
        }
        None => u,
    };
    st.audit(
        &a.user,
        "user_create",
        Some(u.id.to_string()),
        json!({"username": u.username}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(user_dto(&st, u).await?)))
}

/// One account.
#[utoipa::path(
    get,
    path = "/admin/users/{id}",
    params(("id" = i64, Path, description = "User id")),
    responses((status = 200, body = UserDto), (status = 404, body = ErrorBody))
)]
pub async fn get_user(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<UserDto>> {
    let u = st.auth.get_user(id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(user_dto(&st, u).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchUserBody {
    pub username: Option<String>,
    /// Admin password reset; signs the user out everywhere.
    pub password: Option<String>,
    pub role: Option<RoleDto>,
    /// Disabling signs the user out everywhere.
    pub disabled: Option<bool>,
    /// Replaces all limits (omitted fields become unlimited).
    pub limits: Option<LimitsDto>,
    /// Replaces the set of accessible libraries.
    pub library_ids: Option<Vec<i64>>,
    /// Replaces the feature permissions.
    pub permissions: Option<PermissionsDto>,
}

/// Update an account: rename, reset password, set role, disable, set limits, set library access.
#[utoipa::path(
    patch,
    path = "/admin/users/{id}",
    params(("id" = i64, Path, description = "User id")),
    request_body = PatchUserBody,
    responses((status = 200, body = UserDto), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn patch_user(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<PatchUserBody>,
) -> ApiResult<Json<UserDto>> {
    let detail = json!({
        "username": b.username, "role": b.role, "disabled": b.disabled,
        "limits": b.limits.is_some(), "library_ids": b.library_ids,
        "password_reset": b.password.is_some(),
    });
    let u = st
        .auth
        .update_user(
            id,
            UserPatch {
                username: b.username,
                password: b.password,
                role: b.role.map(Into::into),
                disabled: b.disabled,
                limits: b.limits.map(Into::into),
                library_ids: b.library_ids,
                perms: b.permissions.map(Into::into),
            },
        )
        .await?;
    st.audit(&a.user, "user_update", Some(id.to_string()), detail)
        .await;
    Ok(Json(user_dto(&st, u).await?))
}

/// Delete an account (and its sessions, watch state and requests).
#[utoipa::path(
    delete,
    path = "/admin/users/{id}",
    params(("id" = i64, Path, description = "User id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn delete_user(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.delete_user(id).await?;
    st.audit(&a.user, "user_delete", Some(id.to_string()), Value::Null)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Sign a user out of every device.
#[utoipa::path(
    delete,
    path = "/admin/users/{id}/sessions",
    params(("id" = i64, Path, description = "User id")),
    responses((status = 204, description = "Revoked"), (status = 404, body = ErrorBody))
)]
pub async fn revoke_user_sessions(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.get_user(id).await?.ok_or(ApiError::NotFound)?;
    let n = st.auth.revoke_all_sessions(id).await?;
    st.audit(
        &a.user,
        "sessions_revoke",
        Some(id.to_string()),
        json!({"count": n}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────────────────── admin: invites ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct InviteDto {
    pub id: i64,
    pub code: String,
    /// Shareable link: `{public_url}/invite/{code}`.
    pub url: String,
    pub library_ids: Vec<i64>,
    pub limits: LimitsDto,
    /// Permissions of the account the invite creates.
    pub permissions: PermissionsDto,
    pub expires_at: Option<i64>,
    pub used_by: Option<i64>,
    pub used_at: Option<i64>,
    /// `pending` | `used` | `expired`
    pub status: String,
}

fn base_url(st: &AppState, headers: &HeaderMap) -> String {
    if let Some(u) = st.config.public_url.as_deref().filter(|u| !u.is_empty()) {
        return u.trim_end_matches('/').to_string();
    }
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let proto = if st.config.trust_proxy_headers {
        h("x-forwarded-proto").unwrap_or("http")
    } else {
        "http"
    };
    format!("{proto}://{}", h("host").unwrap_or("localhost"))
}

fn invite_dto(base: &str, i: reel_auth::Invite) -> InviteDto {
    let now = reel_auth::signing::now_unix();
    let status = if i.used_by.is_some() {
        "used"
    } else if i.expires_at.is_some_and(|e| e <= now) {
        "expired"
    } else {
        "pending"
    };
    InviteDto {
        url: format!("{base}/invite/{}", i.code),
        id: i.id,
        code: i.code,
        library_ids: i.library_ids,
        limits: i.limits.into(),
        permissions: i.perms.into(),
        expires_at: i.expires_at,
        used_by: i.used_by,
        used_at: i.used_at,
        status: status.into(),
    }
}

/// All invites (newest first).
#[utoipa::path(get, path = "/admin/invites", responses((status = 200, body = Vec<InviteDto>)))]
pub async fn list_invites(
    State(st): State<AppState>,
    _a: AdminUser,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<InviteDto>>> {
    let base = base_url(&st, &headers);
    Ok(Json(
        st.auth
            .list_invites()
            .await?
            .into_iter()
            .map(|i| invite_dto(&base, i))
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateInviteBody {
    /// Libraries the new user may access.
    #[serde(default)]
    pub library_ids: Vec<i64>,
    /// Limits preset for the new user (default: 2 streams, 1 transcode, 8 Mbit/s, 3 movie + 3 show
    /// requests per 7 days).
    pub limits: Option<LimitsDto>,
    /// Feature permissions of the new user (default: none - live TV, VOD, requests and casting
    /// are granted explicitly).
    pub permissions: Option<PermissionsDto>,
    /// Validity in days (default 7; 0 = never expires).
    pub expires_in_days: Option<u32>,
}

/// Create a single-use invite link.
#[utoipa::path(
    post,
    path = "/admin/invites",
    request_body = CreateInviteBody,
    responses((status = 201, body = InviteDto), (status = 400, body = ErrorBody))
)]
pub async fn create_invite(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    headers: HeaderMap,
    Json(b): Json<CreateInviteBody>,
) -> ApiResult<(StatusCode, Json<InviteDto>)> {
    let days = b.expires_in_days.unwrap_or(7);
    let ttl = (days > 0).then(|| std::time::Duration::from_secs(u64::from(days) * 86_400));
    let inv = st
        .auth
        .create_invite_with(
            a.id(),
            b.library_ids,
            b.limits
                .map(Into::into)
                .unwrap_or_else(reel_auth::Limits::invite_default),
            b.permissions.map(Into::into).unwrap_or_default(),
            ttl,
        )
        .await?;
    st.audit(
        &a.user,
        "invite_create",
        Some(inv.id.to_string()),
        json!({"library_ids": inv.library_ids}),
    )
    .await;
    let base = base_url(&st, &headers);
    Ok((StatusCode::CREATED, Json(invite_dto(&base, inv))))
}

/// Revoke an invite.
#[utoipa::path(
    delete,
    path = "/admin/invites/{id}",
    params(("id" = i64, Path, description = "Invite id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody))
)]
pub async fn delete_invite(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.delete_invite(id).await?;
    st.audit(&a.user, "invite_delete", Some(id.to_string()), Value::Null)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ───────────────────────── admin: audit ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditDto {
    pub id: i64,
    pub ts: i64,
    pub actor_id: Option<i64>,
    pub actor_name: Option<String>,
    /// e.g. `login_failure`, `user_create`, `invite_create`, `arr_server_create`.
    pub action: String,
    pub target: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub detail: Option<Value>,
    pub ip: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditQuery {
    /// Page size (default 100, max 500).
    pub limit: Option<i64>,
    /// Return entries older than this audit id.
    pub before: Option<i64>,
}

/// Audit log (newest first): admin actions and login failures/lockouts.
#[utoipa::path(get, path = "/admin/audit", params(AuditQuery), responses((status = 200, body = Vec<AuditDto>)))]
pub async fn audit_log(
    State(st): State<AppState>,
    _a: AdminUser,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditDto>>> {
    let rows = st.auth.list_audit(q.limit.unwrap_or(100), q.before).await?;
    Ok(Json(
        rows.into_iter()
            .map(|e| AuditDto {
                id: e.id,
                ts: e.ts,
                actor_id: e.actor_id,
                actor_name: e.actor_name,
                action: e.action,
                target: e.target,
                detail: e.detail,
                ip: e.ip,
            })
            .collect(),
    ))
}

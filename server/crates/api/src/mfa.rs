//! Second sign-in factor routes (#146): login step two, TOTP, recovery codes and passkeys.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use reel_auth::{AuthError, MfaProof};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, ClientIp, LoginResponse, Peer, login_response, on_admin_network},
    error::{ApiError, ApiResult, ErrorBody},
};

/// A wrong password or code on a signed-in route must not be a 401: clients read that as "your
/// session ended" and sign out.
fn factor_err(e: AuthError) -> ApiError {
    match e {
        AuthError::InvalidCredentials => {
            ApiError::BadRequest("that password or code isn't right".into())
        }
        e => e.into(),
    }
}

/// An admin's sign-in methods change only from the LAN or Tailscale: a stolen admin password used
/// from the internet can't enroll the attacker's own factor and lock the owner out.
fn guard_factor_change(
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

// ───────────────────────── login, step two ─────────────────────────

/// Returned with 202 by `/auth/login` when the account has a second factor.
#[derive(Debug, Serialize, ToSchema)]
pub struct MfaChallengeDto {
    /// Send to `/auth/login/mfa`. It can't be used as a session.
    pub mfa_token: String,
    /// Available methods: `totp`, `passkey`, `recovery`.
    pub methods: Vec<String>,
    pub expires_in_s: u64,
}

impl From<reel_auth::MfaChallenge> for MfaChallengeDto {
    fn from(c: reel_auth::MfaChallenge) -> Self {
        Self {
            mfa_token: c.token,
            methods: c.methods.into_iter().map(str::to_string).collect(),
            expires_in_s: c.expires_in_s,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MfaLoginBody {
    pub mfa_token: String,
    /// 6-digit authenticator code.
    pub code: Option<String>,
    /// Single-use recovery code.
    pub recovery_code: Option<String>,
    /// The browser's WebAuthn assertion.
    #[schema(value_type = Option<Object>)]
    pub passkey: Option<Value>,
}

/// Finish signing in with a second factor. Failures count towards the account's lockout.
#[utoipa::path(
    post,
    path = "/auth/login/mfa",
    security(()),
    request_body = MfaLoginBody,
    responses(
        (status = 200, body = LoginResponse),
        (status = 400, body = ErrorBody),
        (status = 401, description = "Wrong code, or the challenge expired", body = ErrorBody),
        (status = 429, description = "Locked out", body = ErrorBody)
    )
)]
pub async fn login_mfa(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<MfaLoginBody>,
) -> ApiResult<Json<LoginResponse>> {
    let proof = match (b.code, b.recovery_code, b.passkey) {
        (Some(c), None, None) => MfaProof::Totp(c),
        (None, Some(c), None) => MfaProof::Recovery(c),
        (None, None, Some(p)) => MfaProof::Passkey(p),
        _ => {
            return Err(ApiError::BadRequest(
                "send exactly one of code, recovery_code or passkey".into(),
            ));
        }
    };
    let out = st
        .auth
        .complete_mfa(&b.mfa_token, proof, ip.as_deref())
        .await
        .map_err(|e| match e {
            AuthError::InvalidCredentials => ApiError::Unauthorized("that code isn't right".into()),
            AuthError::Unauthorized => {
                ApiError::Unauthorized("this sign-in expired; enter your password again".into())
            }
            e => e.into(),
        })?;
    Ok(Json(login_response(&st, out, peer, &headers).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MfaPasskeyBody {
    pub mfa_token: String,
}

/// WebAuthn options for the passkey option of login step two.
#[utoipa::path(
    post,
    path = "/auth/login/mfa/passkey",
    security(()),
    request_body = MfaPasskeyBody,
    responses((status = 200, body = Object), (status = 401, body = ErrorBody))
)]
pub async fn login_mfa_passkey(
    State(st): State<AppState>,
    Json(b): Json<MfaPasskeyBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(st.auth.challenge_passkey_options(&b.mfa_token).await?))
}

// ───────────────────────── account security ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct PasskeyDto {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SecurityDto {
    /// An authenticator app is active.
    pub totp: bool,
    pub passkeys: Vec<PasskeyDto>,
    pub recovery_codes_left: i64,
    /// The server can do passkeys (`REEL_PUBLIC_URL` is set).
    pub passkeys_available: bool,
    /// This admin has to add a factor before getting admin powers.
    pub must_enroll: bool,
}

/// The current user's sign-in methods.
#[utoipa::path(get, path = "/me/security", responses((status = 200, body = SecurityDto)))]
pub async fn security(State(st): State<AppState>, u: AuthUser) -> ApiResult<Json<SecurityDto>> {
    let s = st.auth.mfa_status(u.id()).await?;
    Ok(Json(SecurityDto {
        must_enroll: st.auth.must_enroll(&u.user).await?,
        totp: s.totp,
        recovery_codes_left: s.recovery_codes_left,
        passkeys_available: s.passkeys_available,
        passkeys: s
            .passkeys
            .into_iter()
            .map(|p| PasskeyDto {
                id: p.id,
                name: p.name,
                created_at: p.created_at,
                last_used_at: p.last_used_at,
            })
            .collect(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReauthBody {
    /// The account's current password.
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TotpStartDto {
    /// Base32 secret, for typing into an authenticator app.
    pub secret: String,
    /// `otpauth://` link, shown as a QR code.
    pub otpauth_uri: String,
}

/// Start adding an authenticator app. It's active once `/me/totp/confirm` gets a valid code.
#[utoipa::path(
    post,
    path = "/me/totp/start",
    request_body = ReauthBody,
    responses((status = 200, body = TotpStartDto), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn totp_start(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<ReauthBody>,
) -> ApiResult<Json<TotpStartDto>> {
    guard_factor_change(&st, &u, peer, &headers)?;
    let e = st
        .auth
        .totp_start(&u.user, &b.password)
        .await
        .map_err(factor_err)?;
    Ok(Json(TotpStartDto {
        secret: e.secret,
        otpauth_uri: e.otpauth_uri,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TotpConfirmBody {
    pub code: String,
}

/// Recovery codes, shown once.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecoveryCodesDto {
    /// Present when new codes were generated. They can't be read again.
    pub recovery_codes: Option<Vec<String>>,
}

/// Activate the authenticator app with its first code.
#[utoipa::path(
    post,
    path = "/me/totp/confirm",
    request_body = TotpConfirmBody,
    responses((status = 200, body = RecoveryCodesDto), (status = 400, body = ErrorBody))
)]
pub async fn totp_confirm(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<TotpConfirmBody>,
) -> ApiResult<Json<RecoveryCodesDto>> {
    guard_factor_change(&st, &u, peer, &headers)?;
    let recovery_codes = st
        .auth
        .totp_confirm(&u.user, &b.code)
        .await
        .map_err(factor_err)?;
    Ok(Json(RecoveryCodesDto { recovery_codes }))
}

/// Remove the authenticator app.
#[utoipa::path(
    post,
    path = "/me/totp/remove",
    request_body = ReauthBody,
    responses((status = 204, description = "Removed"), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn totp_remove(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<ReauthBody>,
) -> ApiResult<StatusCode> {
    guard_factor_change(&st, &u, peer, &headers)?;
    st.auth
        .totp_remove(&u.user, &b.password)
        .await
        .map_err(factor_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Replace all recovery codes.
#[utoipa::path(
    post,
    path = "/me/recovery-codes",
    request_body = ReauthBody,
    responses((status = 200, body = RecoveryCodesDto), (status = 400, body = ErrorBody))
)]
pub async fn new_recovery_codes(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<ReauthBody>,
) -> ApiResult<Json<RecoveryCodesDto>> {
    guard_factor_change(&st, &u, peer, &headers)?;
    let codes = st
        .auth
        .regenerate_recovery_codes(&u.user, &b.password)
        .await
        .map_err(factor_err)?;
    Ok(Json(RecoveryCodesDto {
        recovery_codes: Some(codes),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasskeyStartBody {
    pub password: String,
    /// Label shown in the list, e.g. "MacBook".
    pub name: String,
}

/// Start adding a passkey: options for `navigator.credentials.create()`.
#[utoipa::path(
    post,
    path = "/me/passkeys/start",
    request_body = PasskeyStartBody,
    responses((status = 200, body = Object), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn passkey_start(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<PasskeyStartBody>,
) -> ApiResult<Json<Value>> {
    guard_factor_change(&st, &u, peer, &headers)?;
    Ok(Json(
        st.auth
            .passkey_start(&u.user, &b.password, &b.name)
            .await
            .map_err(factor_err)?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasskeyFinishBody {
    /// The browser's WebAuthn attestation.
    #[schema(value_type = Object)]
    pub credential: Value,
}

/// Store the new passkey.
#[utoipa::path(
    post,
    path = "/me/passkeys/finish",
    request_body = PasskeyFinishBody,
    responses((status = 200, body = RecoveryCodesDto), (status = 400, body = ErrorBody))
)]
pub async fn passkey_finish(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<PasskeyFinishBody>,
) -> ApiResult<Json<RecoveryCodesDto>> {
    guard_factor_change(&st, &u, peer, &headers)?;
    let recovery_codes = st
        .auth
        .passkey_finish(&u.user, b.credential)
        .await
        .map_err(factor_err)?;
    Ok(Json(RecoveryCodesDto { recovery_codes }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasskeyRenameBody {
    pub name: String,
}

/// Rename a passkey.
#[utoipa::path(
    patch,
    path = "/me/passkeys/{id}",
    params(("id" = i64, Path, description = "Passkey id")),
    request_body = PasskeyRenameBody,
    responses((status = 204, description = "Renamed"), (status = 404, body = ErrorBody))
)]
pub async fn passkey_rename(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
    Json(b): Json<PasskeyRenameBody>,
) -> ApiResult<StatusCode> {
    st.auth.passkey_rename(u.id(), id, &b.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove a passkey.
#[utoipa::path(
    post,
    path = "/me/passkeys/{id}/remove",
    params(("id" = i64, Path, description = "Passkey id")),
    request_body = ReauthBody,
    responses((status = 204, description = "Removed"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn passkey_remove(
    State(st): State<AppState>,
    u: AuthUser,
    Peer(peer): Peer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<ReauthBody>,
) -> ApiResult<StatusCode> {
    guard_factor_change(&st, &u, peer, &headers)?;
    st.auth
        .passkey_remove(&u.user, &b.password, id)
        .await
        .map_err(factor_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove all of a user's second factors and sign them out (they lost their device).
#[utoipa::path(
    delete,
    path = "/admin/users/{id}/second-factor",
    params(("id" = i64, Path, description = "User id")),
    responses((status = 204, description = "Reset"), (status = 404, body = ErrorBody))
)]
pub async fn admin_reset_second_factor(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    st.auth.reset_second_factor(id).await?;
    st.audit(&a.user, "mfa_reset", Some(id.to_string()), json!({}))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

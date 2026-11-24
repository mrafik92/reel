//! TV sign-in by code (#147): the TV starts and polls, a signed-in phone approves.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use reel_auth::{AuthError, Device};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::{AuthUser, ClientIp, LoginResponse, Peer, base_url, login_response},
    error::{ApiError, ApiResult, ErrorBody},
};

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeviceStartBody {
    pub device_id: Option<String>,
    /// Shown to the person approving, e.g. "Living Room TV".
    pub device_name: Option<String>,
    pub client: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeviceStartDto {
    /// Secret to poll with. Don't display it.
    pub device_code: String,
    /// Short code to display, e.g. `BCDF-GHJK`.
    pub user_code: String,
    /// Page where the code is approved.
    pub verification_uri: String,
    /// The same page with the code filled in, for a QR code.
    pub verification_uri_complete: String,
    pub expires_in_s: u64,
    /// Seconds to wait between polls.
    pub interval_s: u64,
}

/// Start signing in a TV: returns a code to show and a secret to poll with.
#[utoipa::path(
    post,
    path = "/auth/device/start",
    security(()),
    request_body = DeviceStartBody,
    responses((status = 200, body = DeviceStartDto), (status = 429, description = "Too many pairings", body = ErrorBody))
)]
pub async fn device_start(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(b): Json<DeviceStartBody>,
) -> ApiResult<Json<DeviceStartDto>> {
    let p = st.auth.device_start(Device {
        device_id: b.device_id,
        device_name: b.device_name,
        client: b.client,
        ip,
    })?;
    let link = format!("{}/link", base_url(&st, &headers));
    Ok(Json(DeviceStartDto {
        verification_uri_complete: format!("{link}?code={}", p.user_code),
        verification_uri: link,
        device_code: p.device_code,
        user_code: p.user_code,
        expires_in_s: p.expires_in_s,
        interval_s: p.interval_s,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DevicePollBody {
    pub device_code: String,
}

/// Returned with 202 while nobody has approved the code yet.
#[derive(Debug, Serialize, ToSchema)]
pub struct DevicePendingDto {
    /// Always `pending`.
    pub status: String,
}

/// Poll for approval. The session is delivered once.
#[utoipa::path(
    post,
    path = "/auth/device/poll",
    security(()),
    request_body = DevicePollBody,
    responses(
        (status = 200, description = "Approved", body = LoginResponse),
        (status = 202, description = "Still waiting", body = DevicePendingDto),
        (status = 404, description = "Unknown, expired, refused or already used", body = ErrorBody),
        (status = 429, description = "Polling too fast", body = ErrorBody)
    )
)]
pub async fn device_poll(
    State(st): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    Json(b): Json<DevicePollBody>,
) -> ApiResult<Response> {
    match st.auth.device_poll(&b.device_code).await? {
        Some(out) => Ok(Json(login_response(&st, out, peer, &headers).await?).into_response()),
        None => Ok((
            StatusCode::ACCEPTED,
            Json(DevicePendingDto {
                status: "pending".into(),
            }),
        )
            .into_response()),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeviceLinkBody {
    /// The code shown on the TV.
    pub user_code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeviceLinkInfoDto {
    pub device_name: Option<String>,
    pub client: Option<String>,
    /// Address the request came from, so the person can spot one that isn't theirs.
    pub ip: Option<String>,
    pub expires_in_s: u64,
}

fn link_err(e: AuthError) -> ApiError {
    match e {
        AuthError::NotFound => ApiError::Status(
            StatusCode::NOT_FOUND,
            "that code isn't valid or has expired".into(),
        ),
        e => e.into(),
    }
}

/// What's asking to sign in with this code.
#[utoipa::path(
    post,
    path = "/me/device-link/lookup",
    request_body = DeviceLinkBody,
    responses((status = 200, body = DeviceLinkInfoDto), (status = 404, body = ErrorBody), (status = 429, body = ErrorBody))
)]
pub async fn link_lookup(
    State(st): State<AppState>,
    u: AuthUser,
    Json(b): Json<DeviceLinkBody>,
) -> ApiResult<Json<DeviceLinkInfoDto>> {
    let i = st
        .auth
        .device_lookup(u.id(), &b.user_code)
        .map_err(link_err)?;
    Ok(Json(DeviceLinkInfoDto {
        device_name: i.device_name,
        client: i.client,
        ip: i.ip,
        expires_in_s: i.expires_in_s,
    }))
}

/// Approve the code: that device gets its own session on this account.
#[utoipa::path(
    post,
    path = "/me/device-link/approve",
    request_body = DeviceLinkBody,
    responses((status = 204, description = "Approved"), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn link_approve(
    State(st): State<AppState>,
    u: AuthUser,
    Json(b): Json<DeviceLinkBody>,
) -> ApiResult<StatusCode> {
    st.auth
        .device_approve(&u.user, &b.user_code)
        .await
        .map_err(link_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Refuse the code.
#[utoipa::path(
    post,
    path = "/me/device-link/deny",
    request_body = DeviceLinkBody,
    responses((status = 204, description = "Refused"), (status = 404, body = ErrorBody))
)]
pub async fn link_deny(
    State(st): State<AppState>,
    u: AuthUser,
    Json(b): Json<DeviceLinkBody>,
) -> ApiResult<StatusCode> {
    st.auth
        .device_deny(u.id(), &b.user_code)
        .map_err(link_err)?;
    Ok(StatusCode::NO_CONTENT)
}

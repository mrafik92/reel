use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

/// API error; rendered as `{ "error": "..." }`.
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    BadRequest(String),
    BadGateway(String),
    TooManyRequests(String),
    Unauthorized(String),
    Forbidden(String),
    Locked(std::time::Duration),
    /// Explicit status (e.g. mapped from `reel_requests::Error::http_status`).
    Status(StatusCode, String),
    /// Explicit status with a custom JSON body (must contain `error`).
    Json(StatusCode, serde_json::Value),
    Internal(String),
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            e => Self::Internal(e.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut extra: Option<(header::HeaderName, String)> = None;
        if let Self::Json(code, body) = self {
            return (code, Json(body)).into_response();
        }
        let (code, msg) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::BadGateway(m) => (StatusCode::BAD_GATEWAY, m),
            Self::Unauthorized(m) => {
                extra = Some((header::WWW_AUTHENTICATE, "Bearer".into()));
                (StatusCode::UNAUTHORIZED, m)
            }
            Self::Locked(wait) => {
                extra = Some((header::RETRY_AFTER, wait.as_secs().max(1).to_string()));
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    format!("too many attempts; retry in {} s", wait.as_secs().max(1)),
                )
            }
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::Status(c, m) => (c, m),
            Self::Json(..) => unreachable!("handled above"),
            Self::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m),
            Self::Internal(m) => {
                tracing::error!("internal error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
            }
        };
        let mut res = (code, Json(ErrorBody { error: msg })).into_response();
        if let Some((k, v)) = extra {
            if let Ok(v) = header::HeaderValue::from_str(&v) {
                res.headers_mut().insert(k, v);
            }
        }
        res
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<reel_playback::PlaybackError> for ApiError {
    fn from(e: reel_playback::PlaybackError) -> Self {
        use reel_playback::PlaybackError as P;
        match e {
            P::SessionNotFound => Self::NotFound,
            P::Invalid(m) => Self::BadRequest(m),
            P::GlobalLimit(n) => Self::TooManyRequests(format!(
                "The server's transcode limit ({n} concurrent streams) is reached; try again later"
            )),
            P::UserLimit(n) => Self::TooManyRequests(format!(
                "You reached your limit of {n} concurrent transcoded streams; stop another stream first"
            )),
            e => Self::Internal(e.to_string()),
        }
    }
}

impl From<reel_requests::Error> for ApiError {
    fn from(e: reel_requests::Error) -> Self {
        let code =
            StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        match e {
            reel_requests::Error::Db(e) => Self::Internal(e.to_string()),
            e => Self::Status(code, e.to_string()),
        }
    }
}

impl From<reel_auth::AuthError> for ApiError {
    fn from(e: reel_auth::AuthError) -> Self {
        use reel_auth::AuthError as A;
        match e {
            A::InvalidCredentials => Self::Unauthorized("invalid username or password".into()),
            A::Unauthorized => Self::Unauthorized("invalid or expired token".into()),
            A::Forbidden => Self::Forbidden("forbidden".into()),
            A::Locked(d) => Self::Locked(d),
            A::NotFound => Self::NotFound,
            A::Conflict(m) => Self::Status(StatusCode::CONFLICT, m),
            A::BadRequest(m) => Self::BadRequest(m),
            A::Db(e) => Self::Internal(e.to_string()),
            A::Internal(m) => Self::Internal(m),
        }
    }
}

impl From<reel_iptv::Error> for ApiError {
    fn from(e: reel_iptv::Error) -> Self {
        use reel_iptv::Error as E;
        match e {
            E::NotFound(_) => Self::NotFound,
            E::Invalid(m) | E::Parse(m) => Self::BadRequest(reel_iptv::net::redact_text(&m)),
            e @ E::BudgetExhausted { .. } => Self::TooManyRequests(e.to_string()),
            E::InUseByOthers { max, .. } => Self::TooManyRequests(in_use_message(max)),
            E::Unavailable(m) => Self::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                reel_iptv::net::redact_text(&m),
            ),
            E::Http(e) => Self::BadGateway(reel_iptv::net::redact_text(&e.to_string())),
            E::Db(e) => Self::Internal(e.to_string()),
            E::Io(e) => Self::Internal(e.to_string()),
        }
    }
}

/// Friendly 429 text for non-admins (does not reveal who is watching).
pub fn in_use_message(max: u32) -> String {
    format!(
        "All {max} connection(s) of this provider are in use by other viewers right now. \
         Try again in a moment."
    )
}

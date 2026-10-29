use axum::{
    Json,
    http::StatusCode,
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
        let (code, msg) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::BadGateway(m) => (StatusCode::BAD_GATEWAY, m),
            Self::Internal(m) => {
                tracing::error!("internal error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
            }
        };
        (code, Json(ErrorBody { error: msg })).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

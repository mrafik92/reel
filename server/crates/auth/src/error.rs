use std::time::Duration;

/// Auth errors; the API layer maps them to HTTP statuses.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid or expired token")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("too many attempts; retry in {} s", .0.as_secs().max(1))]
    Locked(Duration),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type AuthResult<T> = Result<T, AuthError>;

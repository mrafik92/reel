//! Crate error type.

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors from the requests crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("arr/download client returned status {status}: {body}")]
    Remote { status: u16, body: String },
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("metadata error: {0}")]
    Metadata(#[from] reel_metadata::Error),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    QuotaExceeded(String),
    #[error("{0}")]
    InvalidState(String),
    #[error("{0}")]
    NotConfigured(String),
    #[error("parse error: {0}")]
    Parse(String),
}

impl Error {
    /// Suggested HTTP status for the API layer.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::Conflict(_) | Error::InvalidState(_) => 409,
            Error::QuotaExceeded(_) => 429,
            Error::NotConfigured(_) => 424,
            Error::Parse(_) => 400,
            Error::Http(_) | Error::Remote { .. } | Error::Metadata(_) => 502,
            Error::Db(_) => 500,
        }
    }
}

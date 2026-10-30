/// Errors of the IPTV crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// The request URL is stripped: it may carry credentials (Xtream `username`/`password`).
    #[error("http error: {0}")]
    Http(reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// The provider's connection limit is reached and every slot has a live viewer.
    #[error(
        "connection limit of source {source_id} reached ({max} concurrent); in use by: {}",
        .watching.join(", ")
    )]
    BudgetExhausted {
        source_id: i64,
        max: u32,
        watching: Vec<String>,
    },
    #[error("{0}")]
    Unavailable(String),
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.without_url())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

//! Metadata: TMDB client, matching, enrichment (TMDB + `.nfo`/local art), image cache.

pub mod enricher;
pub mod images;
pub mod matcher;
pub mod nfo;
pub mod tmdb;

pub use enricher::{Enricher, TmdbEnricher};
pub use images::{ImageCache, ImageKind};
pub use matcher::{Candidate, MatchQuery, best_match, score};
pub use tmdb::TmdbClient;

/// Metadata crate errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// URL stripped: TMDB requests carry `api_key` in the query string.
    #[error("http error: {0}")]
    Http(reqwest::Error),
    #[error("tmdb returned status {0}")]
    Status(u16),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("item {0} not found")]
    ItemNotFound(i64),
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.without_url())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

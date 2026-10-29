//! Shared error type.

/// Convenience result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors produced by Reel crates.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(#[from] Box<figment::Error>),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Other(String),
}

impl From<figment::Error> for Error {
    fn from(e: figment::Error) -> Self {
        Error::Config(Box::new(e))
    }
}

//! Core: domain types, DB pool, migrations, config, error types.

pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod fsperm;
pub mod text;

pub use config::{Config, LibraryConfig};
pub use error::{Error, Result};
pub use events::{RequestState, ServerEvent};

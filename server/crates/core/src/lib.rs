//! Core: domain types, DB pool, migrations, config, error types.

pub mod config;
pub mod db;
pub mod error;
pub mod events;

pub use config::Config;
pub use error::{Error, Result};
pub use events::{RequestState, ServerEvent};

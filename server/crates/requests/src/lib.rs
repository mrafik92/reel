//! Requests: discovery, request workflow, Sonarr/Radarr clients, status tracking, notifications.
//!
//! Entry point is [`RequestsService`]. Construct it with the DB pool, an optional
//! [`reel_metadata::TmdbClient`] and a `tokio::sync::broadcast::Sender<ServerEvent>` (the API
//! layer's event bus), then call [`RequestsService::spawn_tracker`].

pub mod arr;
mod discover;
mod dispatch;
mod error;
pub mod models;
pub mod notify;
mod service;
mod shared;
pub mod state;
pub mod torrent;
mod tracker;
pub mod webhook;

pub use discover::{DiscoverCard, DiscoverDetail, DiscoverPage, Discovery};
pub use error::{Error, Result};
pub use models::{
    ArrKind, ArrServer, ItemProgress, MediaType, Progress, RequestRecord, parse_state, state_str,
};
pub use service::{
    Availability, CreateOptions, ListFilter, Options, RequestsService, WebhookOutcome,
};
pub use shared::TrackerConfig;

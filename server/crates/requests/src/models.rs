//! Shared domain types.

use reel_core::RequestState;
use serde::{Deserialize, Serialize};

/// Requested media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Movie,
    Tv,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Movie => "movie",
            MediaType::Tv => "tv",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "movie" => Some(MediaType::Movie),
            "tv" => Some(MediaType::Tv),
            _ => None,
        }
    }
    pub fn arr_kind(self) -> ArrKind {
        match self {
            MediaType::Movie => ArrKind::Radarr,
            MediaType::Tv => ArrKind::Sonarr,
        }
    }
}

/// Which Arr flavour a server is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrKind {
    Sonarr,
    Radarr,
}

impl ArrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArrKind::Sonarr => "sonarr",
            ArrKind::Radarr => "radarr",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sonarr" => Some(ArrKind::Sonarr),
            "radarr" => Some(ArrKind::Radarr),
            _ => None,
        }
    }
}

/// DB string of a [`RequestState`].
pub fn state_str(s: RequestState) -> &'static str {
    match s {
        RequestState::Requested => "requested",
        RequestState::Approved => "approved",
        RequestState::Searching => "searching",
        RequestState::NotFound => "not_found",
        RequestState::Queued => "queued",
        RequestState::Downloading => "downloading",
        RequestState::Stalled => "stalled",
        RequestState::Importing => "importing",
        RequestState::Available => "available",
        RequestState::Failed => "failed",
        RequestState::Declined => "declined",
    }
}

/// Parse a DB status string.
pub fn parse_state(s: &str) -> Option<RequestState> {
    Some(match s {
        "requested" => RequestState::Requested,
        "approved" => RequestState::Approved,
        "searching" => RequestState::Searching,
        "not_found" => RequestState::NotFound,
        "queued" => RequestState::Queued,
        "downloading" => RequestState::Downloading,
        "stalled" => RequestState::Stalled,
        "importing" => RequestState::Importing,
        "available" => RequestState::Available,
        "failed" => RequestState::Failed,
        "declined" => RequestState::Declined,
        _ => return None,
    })
}

/// States the download tracker polls for.
pub const TRACKED_STATES: &str = "'searching','queued','downloading','stalled','importing'";

/// A request row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestRecord {
    pub id: i64,
    pub user_id: i64,
    pub media_type: MediaType,
    pub tmdb_id: i64,
    /// `None` = every season / a movie.
    pub seasons: Option<Vec<i64>>,
    pub status: RequestState,
    pub arr_kind: Option<ArrKind>,
    /// `arr_servers.id` the request was pushed to.
    pub arr_id: Option<i64>,
    /// Radarr movieId / Sonarr seriesId.
    pub arr_item_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub root_folder: Option<String>,
    pub approved_by: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub note: Option<String>,
    pub title: Option<String>,
    pub poster: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct RequestRow {
    id: i64,
    user_id: i64,
    media_type: String,
    tmdb_id: i64,
    seasons_json: Option<String>,
    status: String,
    arr_kind: Option<String>,
    arr_id: Option<i64>,
    arr_item_id: Option<i64>,
    quality_profile_id: Option<i64>,
    root_folder: Option<String>,
    approved_by: Option<i64>,
    created_at: i64,
    updated_at: i64,
    note: Option<String>,
    title: Option<String>,
    poster: Option<String>,
}

pub(crate) const REQUEST_COLS: &str = "id, user_id, media_type, tmdb_id, seasons_json, status, arr_kind, arr_id, arr_item_id, quality_profile_id, root_folder, approved_by, created_at, updated_at, note, title, poster";

impl From<RequestRow> for RequestRecord {
    fn from(r: RequestRow) -> Self {
        RequestRecord {
            id: r.id,
            user_id: r.user_id,
            media_type: MediaType::parse(&r.media_type).unwrap_or(MediaType::Movie),
            tmdb_id: r.tmdb_id,
            seasons: r
                .seasons_json
                .and_then(|j| serde_json::from_str::<Vec<i64>>(&j).ok()),
            status: parse_state(&r.status).unwrap_or(RequestState::Requested),
            arr_kind: r.arr_kind.as_deref().and_then(ArrKind::parse),
            arr_id: r.arr_id,
            arr_item_id: r.arr_item_id,
            quality_profile_id: r.quality_profile_id,
            root_folder: r.root_folder,
            approved_by: r.approved_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
            note: r.note,
            title: r.title,
            poster: r.poster,
        }
    }
}

/// An `arr_servers` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArrServer {
    pub id: i64,
    pub kind: ArrKind,
    pub name: String,
    pub url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub default_profile: Option<i64>,
    pub default_root: Option<String>,
    pub is_4k: bool,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ArrServerRow {
    id: i64,
    kind: String,
    name: String,
    url: String,
    api_key: String,
    default_profile: Option<i64>,
    default_root: Option<String>,
    is_4k: i64,
}

pub(crate) const ARR_COLS: &str =
    "id, kind, name, url, api_key, default_profile, default_root, is_4k";

impl From<ArrServerRow> for ArrServer {
    fn from(r: ArrServerRow) -> Self {
        ArrServer {
            id: r.id,
            kind: ArrKind::parse(&r.kind).unwrap_or(ArrKind::Radarr),
            name: r.name,
            url: r.url,
            api_key: r.api_key,
            default_profile: r.default_profile,
            default_root: r.default_root,
            is_4k: r.is_4k != 0,
        }
    }
}

/// Per-episode / per-queue-item detail inside a [`Progress`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemProgress {
    pub title: Option<String>,
    pub state: RequestState,
    pub progress: f32,
    pub size_bytes: u64,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub download_id: Option<String>,
}

/// In-memory live progress of a request (not persisted per tick).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub request_id: i64,
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub state: RequestState,
    /// 0..1
    pub progress: f32,
    pub eta_seconds: Option<u64>,
    pub speed_bps: Option<u64>,
    pub size_bytes: Option<u64>,
    pub release_title: Option<String>,
    pub seeds: Option<u32>,
    pub peers: Option<u32>,
    pub error: Option<String>,
    pub items: Vec<ItemProgress>,
}

impl Progress {
    /// Progress with no download detail for a bare state.
    pub fn basic(req: &RequestRecord, state: RequestState) -> Self {
        Progress {
            request_id: req.id,
            tmdb_id: req.tmdb_id,
            media_type: req.media_type,
            state,
            progress: if state == RequestState::Available {
                1.0
            } else {
                0.0
            },
            eta_seconds: None,
            speed_bps: None,
            size_bytes: None,
            release_title: None,
            seeds: None,
            peers: None,
            error: None,
            items: Vec::new(),
        }
    }

    /// The wire event for this progress.
    pub fn to_event(&self) -> reel_core::ServerEvent {
        reel_core::ServerEvent::RequestProgress {
            request_id: self.request_id,
            tmdb_id: self.tmdb_id,
            state: self.state,
            progress: self.progress,
            eta_seconds: self.eta_seconds,
            speed_bps: self.speed_bps,
        }
    }
}

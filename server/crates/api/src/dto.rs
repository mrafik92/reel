//! Phase 1 response shapes (SPEC §4.1). JSON is snake_case.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LibraryKind {
    Movies,
    Shows,
}

impl LibraryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movies => "movies",
            Self::Shows => "shows",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "movies" => Some(Self::Movies),
            "shows" => Some(Self::Shows),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Movie,
    Show,
    Season,
    Episode,
}

impl ItemKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "show" => Self::Show,
            "season" => Self::Season,
            "episode" => Self::Episode,
            _ => Self::Movie,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Hdr {
    None,
    Hdr10,
    Hlg,
    Dv,
}

impl Hdr {
    pub fn parse(s: &str) -> Self {
        match s {
            "hdr10" => Self::Hdr10,
            "hlg" => Self::Hlg,
            "dv" => Self::Dv,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Library {
    pub id: i64,
    pub name: String,
    pub kind: LibraryKind,
    pub paths: Vec<String>,
    /// Top-level items (movies or shows).
    pub item_count: i64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateLibrary {
    pub name: String,
    pub kind: LibraryKind,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ItemSummary {
    pub id: i64,
    pub kind: ItemKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_no: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_no: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    /// Watch progress 0..1 (movies/episodes with a saved position).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    pub played: bool,
    /// Unix epoch seconds.
    pub added_at: i64,
    /// Community rating 0–10 (list endpoints only; see `ItemDetail.rating` for the detail view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Runtime in seconds (list endpoints only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_s: Option<i64>,
    /// Genre names (list endpoints only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    /// The series an episode belongs to (only on Home's `continue_watching`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<SeriesRef>,
}

/// The series identity of an episode in Continue watching, so a client can show the title and art
/// of the series while Play still resumes the episode.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SeriesRef {
    pub id: i64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoStream {
    pub codec: String,
    pub width: i64,
    pub height: i64,
    pub hdr: Hdr,
    pub bit_depth: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AudioStream {
    pub index: i64,
    pub codec: String,
    pub channels: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubtitleStream {
    pub index: i64,
    pub codec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub forced: bool,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MediaFile {
    pub id: i64,
    pub container: String,
    pub duration_s: f64,
    pub size: i64,
    /// `/api/v1/direct/{id}`
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub subtitles: Vec<SubtitleStream>,
    /// Multi-episode files (`S01E02E03`): ids of every episode item stored in this file, in
    /// episode order. Absent for ordinary files. Pass the episode being watched as `item_id` to
    /// `/playback/{file}/decide` so progress is credited to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CastMember {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
    /// TMDB person id; open `/people/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_tmdb_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ItemDetail {
    #[serde(flatten)]
    pub summary: ItemSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_s: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    pub cast: Vec<CastMember>,
    pub children_count: i64,
    pub files: Vec<MediaFile>,
    /// YouTube id of the title's main trailer (movies and shows, from TMDB); embed it from
    /// `youtube-nocookie.com`. Absent when TMDB has none or it hasn't been looked up yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailer_youtube_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ItemPage {
    pub items: Vec<ItemSummary>,
    /// Opaque cursor for the next page; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HomeRow {
    pub library_id: i64,
    pub title: String,
    pub items: Vec<ItemSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Home {
    pub continue_watching: Vec<ItemSummary>,
    pub recently_added: Vec<ItemSummary>,
    pub rows: Vec<HomeRow>,
}

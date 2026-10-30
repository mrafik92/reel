//! People pages, ratings & reviews, and the all-Arr downloads view.

use axum::{
    Json,
    extract::{Path, State},
};
use reel_core::RequestState;
use reel_requests::{
    MediaType,
    downloads::DownloadItem,
    reviews::{ExternalLink, Rating, Review, ReviewsService},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::AuthUser,
    error::{ApiError, ApiResult, ErrorBody},
    requests::{DiscoverCardDto, card_dto, tmdb_image},
    routes::image_url,
};

// ───────────────────────── people ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PersonCreditDto {
    /// The same card shape as discovery (status, progress, `library_item_id`, …).
    #[serde(flatten)]
    pub card: DiscoverCardDto,
    /// `cast` | `crew`
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PersonDto {
    pub tmdb_id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deathday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_of_birth: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_for_department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    /// Filmography, newest first (cast preferred over crew for the same title).
    pub credits: Vec<PersonCreditDto>,
}

/// A person from TMDB with their filmography as discovery cards.
#[utoipa::path(
    get,
    path = "/people/{tmdb_id}",
    params(("tmdb_id" = i64, Path, description = "TMDB person id")),
    responses(
        (status = 200, body = PersonDto),
        (status = 404, body = ErrorBody),
        (status = 424, description = "TMDB not configured", body = ErrorBody)
    )
)]
pub async fn get_person(
    State(st): State<AppState>,
    u: AuthUser,
    Path(tmdb_id): Path<i64>,
) -> ApiResult<Json<PersonDto>> {
    let p = st.requests.discovery().person(tmdb_id).await?;
    let sg = st.signer(u.id());
    Ok(Json(PersonDto {
        tmdb_id: p.tmdb_id,
        name: p.name,
        biography: p.biography,
        birthday: p.birthday,
        deathday: p.deathday,
        place_of_birth: p.place_of_birth,
        known_for_department: p.known_for_department,
        photo_url: tmdb_image(&sg, "profile", &p.profile_path, 342),
        imdb_id: p.imdb_id,
        credits: p
            .credits
            .iter()
            .map(|c| PersonCreditDto {
                card: card_dto(&c.card, &sg),
                role: c.role.to_string(),
                character: c.character.clone(),
                job: c.job.clone(),
            })
            .collect(),
    }))
}

// ───────────────────────── reviews ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RatingDto {
    /// `imdb` | `rotten_tomatoes` | `metacritic` | `tmdb`
    pub source: String,
    pub label: String,
    /// Display value, e.g. `7.8/10`, `94%`.
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_100: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReviewDto {
    /// `tmdb` | `trakt`
    pub source: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// 0..10
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Plain text / light markdown.
    pub content: String,
    /// ISO-8601 timestamp.
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReviewLinkDto {
    /// `imdb` | `rotten_tomatoes` | `letterboxd` | `tmdb` | `trakt`
    pub source: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReviewsDto {
    pub ratings: Vec<RatingDto>,
    pub reviews: Vec<ReviewDto>,
    pub links: Vec<ReviewLinkDto>,
}

fn rating_dto(r: &Rating) -> RatingDto {
    RatingDto {
        source: r.source.into(),
        label: r.label.clone(),
        value: r.value.clone(),
        score_100: r.score_100,
        votes: r.votes,
        url: r.url.clone(),
    }
}

fn review_dto(r: &Review, sg: &reel_auth::UrlSigner) -> ReviewDto {
    ReviewDto {
        source: r.source.into(),
        author: r.author.clone(),
        avatar_url: tmdb_image(sg, "profile", &r.avatar_path, 64),
        rating: r.rating,
        content: r.content.clone(),
        created_at: r.created_at.clone(),
        url: r.url.clone(),
    }
}

fn link_dto(l: &ExternalLink) -> ReviewLinkDto {
    ReviewLinkDto {
        source: l.source.into(),
        url: l.url.clone(),
    }
}

/// Ratings (TMDB; IMDb / Rotten Tomatoes / Metacritic with an OMDb key), user reviews (TMDB; Trakt
/// with a client id) and outbound links for a title.
#[utoipa::path(
    get,
    path = "/reviews/{media}/{tmdb_id}",
    params(
        ("media" = String, Path, description = "movie | tv"),
        ("tmdb_id" = i64, Path, description = "TMDB id")
    ),
    responses(
        (status = 200, body = ReviewsDto),
        (status = 404, body = ErrorBody),
        (status = 424, description = "TMDB not configured", body = ErrorBody)
    )
)]
pub async fn get_reviews(
    State(st): State<AppState>,
    u: AuthUser,
    Path((media, tmdb_id)): Path<(String, i64)>,
) -> ApiResult<Json<ReviewsDto>> {
    let media = MediaType::parse(&media).ok_or(ApiError::NotFound)?;
    let tmdb = st.tmdb.clone().ok_or_else(|| {
        ApiError::from(reel_requests::Error::NotConfigured(
            "TMDB is not configured".into(),
        ))
    })?;
    let svc = ReviewsService::new(st.db.clone(), tmdb, st.omdb.clone(), st.trakt.clone());
    let b = svc.get(media, tmdb_id).await?;
    let sg = st.signer(u.id());
    Ok(Json(ReviewsDto {
        ratings: b.ratings.iter().map(rating_dto).collect(),
        reviews: b.reviews.iter().map(|r| review_dto(r, &sg)).collect(),
        links: b.links.iter().map(link_dto).collect(),
    }))
}

// ───────────────────────── downloads ─────────────────────────

/// One Radarr/Sonarr queue record (also the `items` of the `downloads` WebSocket event).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DownloadDto {
    /// `"{arr_server_id}:{queue_id}"`
    pub id: String,
    pub arr_server_id: i64,
    pub arr_name: String,
    /// `movie` | `tv`
    pub media_type: crate::requests::MediaTypeDto,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    /// queued | downloading | stalled | importing | failed
    pub state: RequestState,
    /// 0..1
    pub progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizeleft_bytes: Option<u64>,
    /// Needs a torrent client configured (admin → download clients).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Most specific human-readable reason (Arr message, else torrent-client state such as
    /// "Fetching metadata", "Stalled — no peers", "Queued in client", "Paused").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    /// Raw torrent-client state (`metaDL`, `stalledDL`, …); needs a download client configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_state: Option<String>,
    /// Reel request this belongs to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_item_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
}

/// Items visible to `user` (admins: everything; others: only downloads of their own requests),
/// with signed poster URLs.
pub(crate) async fn downloads_for(
    st: &AppState,
    user: &reel_auth::User,
    items: &[DownloadItem],
) -> Vec<DownloadDto> {
    let sg = st.signer(user.id);
    let mut out = Vec::with_capacity(items.len());
    let mut lib_ok: std::collections::HashMap<i64, bool> = Default::default();
    for i in items {
        if !user.is_admin() && i.request_user_id != Some(user.id) {
            continue;
        }
        // Library link / poster only when the user may see that library item.
        let mut lib = i.library_item_id;
        if let (Some(id), false) = (lib, user.is_admin()) {
            let ok = match lib_ok.get(&id) {
                Some(o) => *o,
                None => {
                    let o = st.require_item(user, id).await.is_ok();
                    lib_ok.insert(id, o);
                    o
                }
            };
            if !ok {
                lib = None;
            }
        }
        let has_lib_poster = match lib {
            Some(id) => sqlx::query_scalar::<_, Option<String>>(
                "SELECT poster FROM media_items WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten()
            .flatten()
            .is_some(),
            None => false,
        };
        let poster_url = match (lib, has_lib_poster) {
            (Some(id), true) => Some(image_url(&sg, "poster", id, 342)),
            _ => tmdb_image(&sg, "poster", &i.poster_path, 342),
        };
        out.push(DownloadDto {
            id: i.id.clone(),
            arr_server_id: i.arr_server_id,
            arr_name: i.arr_name.clone(),
            media_type: i.media_type.into(),
            title: i.title.clone(),
            release_title: i.release_title.clone(),
            tmdb_id: i.tmdb_id,
            tvdb_id: i.tvdb_id,
            season: i.season,
            episode: i.episode,
            episode_title: i.episode_title.clone(),
            state: i.state,
            progress: i.progress,
            eta_seconds: i.eta_seconds,
            size_bytes: i.size_bytes,
            sizeleft_bytes: i.sizeleft_bytes,
            speed_bps: i.speed_bps,
            error: i.error.clone(),
            status_text: i.status_text.clone(),
            client_state: i.client_state.clone(),
            request_id: i.request_id,
            library_item_id: lib,
            poster_url,
        });
    }
    out
}

/// Everything in the Radarr/Sonarr queues (including downloads not started from Reel). Admins see
/// all; other users only downloads of their own requests. Snapshot cached ~2 s server-side; the
/// same data is pushed as `{"type":"downloads","items":[…]}` on `/events` while anything is active.
#[utoipa::path(get, path = "/downloads", responses((status = 200, body = Vec<DownloadDto>)))]
pub async fn list_downloads(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<Vec<DownloadDto>>> {
    let items = st.requests.downloads().await?;
    Ok(Json(downloads_for(&st, &u.user, &items).await))
}

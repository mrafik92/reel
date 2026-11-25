//! Discovery, requests workflow, Arr webhooks and admin configuration (SPEC §7).

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
};
use reel_core::RequestState;
use reel_requests::{
    CreateOptions, DiscoverCard, ListFilter, MediaType, Progress,
    arr::ArrClient,
    notify::{Notification, NotifyKind, Target},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, ClientIp},
    error::{ApiError, ApiResult, ErrorBody},
};

// ───────────────────────── DTOs ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaTypeDto {
    Movie,
    Tv,
}

impl From<MediaType> for MediaTypeDto {
    fn from(m: MediaType) -> Self {
        match m {
            MediaType::Movie => Self::Movie,
            MediaType::Tv => Self::Tv,
        }
    }
}

impl From<MediaTypeDto> for MediaType {
    fn from(m: MediaTypeDto) -> Self {
        match m {
            MediaTypeDto::Movie => Self::Movie,
            MediaTypeDto::Tv => Self::Tv,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Partially,
    Requested,
    None,
}

/// Per-item progress inside a request (e.g. one episode / one release).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ItemProgressDto {
    pub title: Option<String>,
    pub state: RequestState,
    pub progress: f32,
    pub size_bytes: u64,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub download_id: Option<String>,
}

/// Live progress of a request (same data as the `request_progress` WebSocket event, plus detail).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProgressDto {
    pub request_id: i64,
    pub tmdb_id: i64,
    pub media_type: MediaTypeDto,
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
    pub items: Vec<ItemProgressDto>,
}

/// Convert between structurally identical serde types.
fn via<T: serde::de::DeserializeOwned>(v: &impl Serialize) -> T {
    serde_json::from_value(serde_json::to_value(v).unwrap_or(Value::Null))
        .expect("DTO mirrors the reel-requests type")
}

fn progress_dto(p: &Progress) -> ProgressDto {
    via(p)
}

/// `/api/v1/images/tmdb/{kind}?path=…&w=…`
pub(crate) fn tmdb_image(
    sg: &reel_auth::UrlSigner,
    kind: &str,
    path: &Option<String>,
    w: u32,
) -> Option<String> {
    path.as_ref().map(|p| {
        sg.url(
            &format!(
                "/api/v1/images/tmdb/{kind}?path={}&w={w}",
                p.replace('/', "%2F")
            ),
            &format!("tmdb:{kind}:{p}"),
            2 * 86_400,
            86_400,
        )
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoverCardDto {
    pub tmdb_id: i64,
    pub media_type: MediaTypeDto,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub vote_average: Option<f64>,
    /// Library availability badge.
    pub status: Availability,
    /// Live download progress when a request is in flight.
    pub progress: Option<ProgressDto>,
    /// Library item (movie or show) holding this title, when it is in the library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_item_id: Option<i64>,
    /// Xtream VOD catalog entry for this title (play it from `/vod/movies/{id}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vod_movie_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vod_series_id: Option<i64>,
}

pub(crate) fn card_dto(c: &DiscoverCard, sg: &reel_auth::UrlSigner) -> DiscoverCardDto {
    DiscoverCardDto {
        tmdb_id: c.tmdb_id,
        media_type: c.media_type.into(),
        title: c.title.clone(),
        year: c.year,
        overview: c.overview.clone(),
        poster_url: tmdb_image(sg, "poster", &c.poster_path, 342),
        backdrop_url: tmdb_image(sg, "backdrop", &c.backdrop_path, 780),
        vote_average: c.vote_average,
        status: via(&c.status),
        progress: c.progress.as_ref().map(progress_dto),
        library_item_id: c.library_item_id,
        vod_movie_id: c.vod_movie_id,
        vod_series_id: c.vod_series_id,
    }
}

/// Library items `user` may open, by TMDB id: movies with a file, or shows, in a visible library
/// (lowest id per title). `None` = the user sees every library (admins), no lookup needed.
pub(crate) async fn visible_library_items(
    st: &AppState,
    user: &reel_auth::User,
    media: MediaType,
    tmdb_ids: &[i64],
) -> ApiResult<Option<std::collections::HashMap<i64, i64>>> {
    let Some(vis) = st.visible(user).await? else {
        return Ok(None);
    };
    if tmdb_ids.is_empty() || vis.is_empty() {
        return Ok(Some(Default::default()));
    }
    let kind = match media {
        MediaType::Movie => "movie",
        MediaType::Tv => "show",
    };
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT i.tmdb_id, MIN(i.id) FROM media_items i
         WHERE i.kind = ?1 AND i.tmdb_id IN (SELECT value FROM json_each(?2))
           AND i.library_id IN (SELECT value FROM json_each(?3))
           AND (i.kind != 'movie' OR EXISTS (SELECT 1 FROM media_files f WHERE f.item_id = i.id))
         GROUP BY i.tmdb_id",
    )
    .bind(kind)
    .bind(serde_json::to_string(tmdb_ids).unwrap_or_else(|_| "[]".into()))
    .bind(serde_json::to_string(&vis).unwrap_or_else(|_| "[]".into()))
    .fetch_all(&st.db)
    .await?;
    Ok(Some(rows.into_iter().collect()))
}

/// Re-point each card's `library_item_id` at a copy of the title `user` may open, or drop it. The
/// discovery service links the first copy server-wide, which can sit in a library the user has no
/// access to; the id alone would reveal that the title is there (and its item id).
pub(crate) async fn scope_cards<'a>(
    st: &AppState,
    user: &reel_auth::User,
    cards: impl IntoIterator<Item = &'a mut DiscoverCardDto>,
) -> ApiResult<()> {
    let mut cards: Vec<&mut DiscoverCardDto> = cards
        .into_iter()
        .filter(|c| c.library_item_id.is_some())
        .collect();
    for media in [MediaTypeDto::Movie, MediaTypeDto::Tv] {
        let ids: Vec<i64> = cards
            .iter()
            .filter(|c| c.media_type == media)
            .map(|c| c.tmdb_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let Some(map) = visible_library_items(st, user, media.into(), &ids).await? else {
            return Ok(());
        };
        for c in cards.iter_mut().filter(|c| c.media_type == media) {
            c.library_item_id = map.get(&c.tmdb_id).copied();
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoverPageDto {
    pub page: u32,
    pub total_pages: u32,
    pub total_results: u32,
    pub results: Vec<DiscoverCardDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoverDetailDto {
    pub card: DiscoverCardDto,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    /// Season numbers (TV; specials excluded).
    pub seasons: Vec<i64>,
    /// Top-billed cast (max 20).
    pub cast: Vec<crate::dto::CastMember>,
    /// YouTube id of the title's main trailer on TMDB; embed it from `youtube-nocookie.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailer_youtube_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RequestDto {
    pub id: i64,
    pub user_id: i64,
    pub media_type: MediaTypeDto,
    pub tmdb_id: i64,
    /// TV only; absent = all seasons.
    pub seasons: Option<Vec<i64>>,
    pub status: RequestState,
    /// `sonarr` | `radarr`
    pub arr_kind: Option<String>,
    pub arr_server_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub root_folder: Option<String>,
    pub approved_by: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub note: Option<String>,
    pub title: Option<String>,
    pub poster_url: Option<String>,
}

fn request_dto(r: &reel_requests::RequestRecord, sg: &reel_auth::UrlSigner) -> RequestDto {
    RequestDto {
        id: r.id,
        user_id: r.user_id,
        media_type: r.media_type.into(),
        tmdb_id: r.tmdb_id,
        seasons: r.seasons.clone(),
        status: r.status,
        arr_kind: r.arr_kind.map(|k| k.as_str().to_string()),
        arr_server_id: r.arr_id,
        quality_profile_id: r.quality_profile_id,
        root_folder: r.root_folder.clone(),
        approved_by: r.approved_by,
        created_at: r.created_at,
        updated_at: r.updated_at,
        note: r.note.clone(),
        title: r.title.clone(),
        poster_url: tmdb_image(sg, "poster", &r.poster, 342),
    }
}

// ───────────────────────── discovery ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct DiscoverQuery {
    /// `movie`, `tv` or `all` (default).
    pub media: Option<String>,
    /// 1-based page (default 1).
    pub page: Option<u32>,
}

fn parse_media(s: Option<&str>) -> ApiResult<Option<MediaType>> {
    match s.unwrap_or("all") {
        "all" => Ok(None),
        other => MediaType::parse(other)
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest(format!("unknown media '{other}'"))),
    }
}

fn page_dto(p: reel_requests::DiscoverPage, sg: &reel_auth::UrlSigner) -> DiscoverPageDto {
    DiscoverPageDto {
        page: p.page,
        total_pages: p.total_pages,
        total_results: p.total_results,
        results: p.results.iter().map(|c| card_dto(c, sg)).collect(),
    }
}

/// Browse TMDB lists: `trending`, `popular` or `upcoming` (movies only).
#[utoipa::path(
    get,
    path = "/discover/{list}",
    params(("list" = String, Path, description = "trending | popular | upcoming"), DiscoverQuery),
    responses(
        (status = 200, body = DiscoverPageDto),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 424, description = "TMDB not configured", body = ErrorBody)
    )
)]
pub async fn discover_list(
    State(st): State<AppState>,
    u: AuthUser,
    Path(list): Path<String>,
    Query(q): Query<DiscoverQuery>,
) -> ApiResult<Json<DiscoverPageDto>> {
    let media = parse_media(q.media.as_deref())?;
    let page = q.page.unwrap_or(1).max(1);
    let d = st.requests.discovery();
    let out = match list.as_str() {
        "trending" => {
            let m = media.map_or("all", MediaType::as_str);
            d.trending(m, "week", page).await?
        }
        "popular" => match media {
            Some(m) => d.popular(m, page).await?,
            None => {
                let (mut a, b) = (
                    d.popular(MediaType::Movie, page).await?,
                    d.popular(MediaType::Tv, page).await?,
                );
                a.results.extend(b.results);
                a.results.sort_by(|x, y| {
                    y.vote_average
                        .partial_cmp(&x.vote_average)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                a.total_pages = a.total_pages.max(b.total_pages);
                a.total_results += b.total_results;
                a
            }
        },
        "upcoming" => d.upcoming(page).await?,
        _ => return Err(ApiError::NotFound),
    };
    let mut dto = page_dto(out, &st.signer(u.id()));
    scope_cards(&st, &u.user, &mut dto.results).await?;
    Ok(Json(dto))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DiscoverSearchQuery {
    /// Search text.
    pub q: Option<String>,
    /// `movie`, `tv` or `all` (default).
    pub media: Option<String>,
}

/// Search TMDB; every card carries an availability badge and live download progress.
#[utoipa::path(
    get,
    path = "/discover/search",
    params(DiscoverSearchQuery),
    responses(
        (status = 200, body = Vec<DiscoverCardDto>),
        (status = 424, description = "TMDB not configured", body = ErrorBody)
    )
)]
pub async fn discover_search(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<DiscoverSearchQuery>,
) -> ApiResult<Json<Vec<DiscoverCardDto>>> {
    let text = q.q.unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let media = parse_media(q.media.as_deref())?;
    let cards = st.requests.discovery().search(text, media).await?;
    let sg = st.signer(u.id());
    let mut out: Vec<DiscoverCardDto> = cards.iter().map(|c| card_dto(c, &sg)).collect();
    scope_cards(&st, &u.user, &mut out).await?;
    Ok(Json(out))
}

/// TMDB detail card (genres, runtime, seasons) with availability + progress.
#[utoipa::path(
    get,
    path = "/discover/{media}/{tmdb_id}",
    params(
        ("media" = String, Path, description = "movie | tv"),
        ("tmdb_id" = i64, Path, description = "TMDB id")
    ),
    responses(
        (status = 200, body = DiscoverDetailDto),
        (status = 404, body = ErrorBody),
        (status = 424, description = "TMDB not configured", body = ErrorBody)
    )
)]
pub async fn discover_detail(
    State(st): State<AppState>,
    u: AuthUser,
    Path((media, tmdb_id)): Path<(String, i64)>,
) -> ApiResult<Json<DiscoverDetailDto>> {
    let media = MediaType::parse(&media).ok_or(ApiError::NotFound)?;
    let d = st.requests.discovery().detail(media, tmdb_id).await?;
    let mut card = card_dto(&d.card, &st.signer(u.id()));
    scope_cards(&st, &u.user, [&mut card]).await?;
    Ok(Json(DiscoverDetailDto {
        card,
        genres: d.genres,
        runtime_minutes: d.runtime_minutes,
        imdb_id: d.imdb_id,
        tvdb_id: d.tvdb_id,
        seasons: d.seasons,
        cast: {
            let sg = st.signer(u.id());
            d.cast
                .iter()
                .map(|c| crate::dto::CastMember {
                    name: c.name.clone(),
                    character: c.character.clone(),
                    photo_url: tmdb_image(&sg, "profile", &c.profile_path, 185),
                    person_tmdb_id: Some(c.person_tmdb_id),
                })
                .collect()
        },
        trailer_youtube_id: d.trailer_youtube_id,
    }))
}

// ───────────────────────── requests ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct RequestsQuery {
    pub status: Option<RequestState>,
    /// Page size (default 100, max 1000).
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// List requests (newest first).
#[utoipa::path(get, path = "/requests", params(RequestsQuery), responses((status = 200, body = Vec<RequestDto>)))]
pub async fn list_requests(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<RequestsQuery>,
) -> ApiResult<Json<Vec<RequestDto>>> {
    // Non-admins only see their own requests.
    let rows = st
        .requests
        .list(&ListFilter {
            status: q.status,
            user_id: (!u.is_admin()).then(|| u.id()),
            limit: q.limit,
            offset: q.offset,
        })
        .await?;
    let sg = st.signer(u.id());
    Ok(Json(rows.iter().map(|r| request_dto(r, &sg)).collect()))
}

/// Snapshot of every unfinished request with its latest progress (call on app load / reconnect,
/// then follow `request_progress` events on `/events`).
#[utoipa::path(get, path = "/requests/active", responses((status = 200, body = Vec<ProgressDto>)))]
pub async fn active_requests(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<Vec<ProgressDto>>> {
    let snap = st.requests.active_snapshot().await?;
    let own: std::collections::HashSet<i64> = if u.is_admin() {
        Default::default()
    } else {
        sqlx::query_scalar("SELECT id FROM requests WHERE user_id = ?")
            .bind(u.id())
            .fetch_all(&st.db)
            .await?
            .into_iter()
            .collect()
    };
    Ok(Json(
        snap.iter()
            .filter(|p| u.is_admin() || own.contains(&p.request_id))
            .map(progress_dto)
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRequestBody {
    pub media_type: MediaTypeDto,
    pub tmdb_id: i64,
    /// TV only; omit for all seasons.
    pub seasons: Option<Vec<i64>>,
    pub arr_server_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub root_folder: Option<String>,
}

/// Create a request. Admins / auto-approve users are approved and pushed to Radarr/Sonarr at once.
#[utoipa::path(
    post,
    path = "/requests",
    request_body = CreateRequestBody,
    responses(
        (status = 201, body = RequestDto),
        (status = 409, description = "Already available or requested", body = ErrorBody),
        (status = 424, description = "No Radarr/Sonarr configured", body = ErrorBody),
        (status = 429, description = "Quota exceeded", body = ErrorBody)
    )
)]
pub async fn create_request(
    State(st): State<AppState>,
    u: AuthUser,
    Json(b): Json<CreateRequestBody>,
) -> ApiResult<(StatusCode, Json<RequestDto>)> {
    // routing a request to a particular Radarr/Sonarr, quality profile or folder is an admin's
    // call: everyone else's values are ignored (the configured defaults apply)
    let admin = u.is_admin();
    let r = st
        .requests
        .create_request_with(
            u.id(),
            b.media_type.into(),
            b.tmdb_id,
            CreateOptions {
                seasons: b.seasons,
                arr_server_id: b.arr_server_id.filter(|_| admin),
                quality_profile_id: b.quality_profile_id.filter(|_| admin),
                root_folder: b.root_folder.filter(|_| admin),
            },
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(request_dto(&r, &st.signer(u.id()))),
    ))
}

/// Approve a pending request and push it to Radarr/Sonarr.
#[utoipa::path(
    post,
    path = "/requests/{id}/approve",
    params(("id" = i64, Path, description = "Request id")),
    responses((status = 200, body = RequestDto), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn approve_request(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<RequestDto>> {
    let r = st.requests.approve(id, a.id()).await?;
    st.audit(
        &a.user,
        "request_approve",
        Some(id.to_string()),
        serde_json::Value::Null,
    )
    .await;
    Ok(Json(request_dto(&r, &st.signer(a.id()))))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct DeclineBody {
    pub note: Option<String>,
}

/// Decline a pending request.
#[utoipa::path(
    post,
    path = "/requests/{id}/decline",
    params(("id" = i64, Path, description = "Request id")),
    request_body(content = DeclineBody, description = "Optional note"),
    responses((status = 200, body = RequestDto), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn decline_request(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    body: Option<Json<DeclineBody>>,
) -> ApiResult<Json<RequestDto>> {
    let note = body.and_then(|b| b.0.note);
    let r = st.requests.decline(id, a.id(), note).await?;
    st.audit(
        &a.user,
        "request_decline",
        Some(id.to_string()),
        serde_json::Value::Null,
    )
    .await;
    Ok(Json(request_dto(&r, &st.signer(a.id()))))
}

/// Search again for a `not_found` / `failed` / `stalled` request.
#[utoipa::path(
    post,
    path = "/requests/{id}/retry",
    params(("id" = i64, Path, description = "Request id")),
    responses((status = 200, body = RequestDto), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn retry_request(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<RequestDto>> {
    // Owner or admin.
    let cur = st.requests.get(id).await?;
    if cur.user_id != u.id() && !u.is_admin() {
        return Err(ApiError::NotFound);
    }
    let r = st.requests.retry(id).await?;
    Ok(Json(request_dto(&r, &st.signer(u.id()))))
}

// ───────────────────────── Arr webhook ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct WebhookQuery {
    /// Arr server id (`arr_servers.id`), to disambiguate multiple instances.
    pub server: Option<i64>,
    /// Deprecated: shared secret in the URL (leaks into logs). Send it as the `X-Reel-Token`
    /// header, a Bearer token or the HTTP Basic password instead. Must be at least 32 characters.
    pub token: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookResult {
    /// Webhook event type as understood by the server (`Grab`, `Download`, …).
    pub event: String,
    /// Requests whose state changed.
    pub request_ids: Vec<i64>,
    /// Paths that were re-scanned into the library.
    pub scanned_paths: Vec<String>,
}

/// Constant-time comparison that does not leak the secret's length (both sides are hashed first).
fn secrets_match(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let (x, y) = (Sha256::digest(a.as_bytes()), Sha256::digest(b.as_bytes()));
    x.iter()
        .zip(y.iter())
        .fold(0u8, |acc, (p, q)| acc | (p ^ q))
        == 0
}

/// Minimum length of `webhook_secret`; shorter secrets keep webhooks disabled.
pub const MIN_WEBHOOK_SECRET: usize = 32;

/// Every token a webhook caller presented: `X-Reel-Token`, `Authorization: Bearer <t>`, HTTP
/// Basic (the password part: Radarr/Sonarr "Connection" username/password) and the deprecated
/// `?token=` (visible in access logs and proxies). Any one matching is enough.
fn presented_tokens(headers: &HeaderMap, query: Option<&str>) -> Vec<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut out = vec![];
    if let Some(t) = headers.get("x-reel-token").and_then(|v| v.to_str().ok()) {
        out.push(t.trim().to_string());
    }
    if let Some(a) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = a.strip_prefix("Bearer ") {
            out.push(t.trim().to_string());
        }
        if let Some(b) = a.strip_prefix("Basic ") {
            if let Some(s) = STANDARD
                .decode(b.trim())
                .ok()
                .and_then(|r| String::from_utf8(r).ok())
            {
                if let Some((_, p)) = s.split_once(':') {
                    out.push(p.to_string());
                }
            }
        }
    }
    out.extend(query.map(str::to_string));
    out
}

/// Canonicalize a webhook path so it matches library roots (which are stored canonicalized);
/// removed files fall back to their canonical parent directory.
fn resolve_path(p: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(p);
    // `..` (or a relative path) has no business in a webhook: it is how a path escapes a library root
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    if let Ok(c) = std::fs::canonicalize(path) {
        return Some(c);
    }
    match (
        path.parent().and_then(|d| std::fs::canonicalize(d).ok()),
        path.file_name(),
    ) {
        (Some(d), Some(f)) => Some(d.join(f)),
        _ => None,
    }
}

/// Radarr/Sonarr webhook receiver. Imported/removed files trigger a targeted library rescan.
#[utoipa::path(
    post,
    path = "/webhooks/arr",
    security(()),
    params(WebhookQuery),
    request_body(content = Value, description = "Radarr/Sonarr webhook payload"),
    responses(
        (status = 200, body = WebhookResult),
        (status = 400, body = ErrorBody),
        (status = 403, description = "Bad or missing token / webhooks disabled", body = ErrorBody)
    )
)]
pub async fn arr_webhook(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(q): Query<WebhookQuery>,
    body: Bytes,
) -> ApiResult<Json<WebhookResult>> {
    let Some(secret) = st
        .config
        .webhook_secret
        .as_deref()
        .filter(|s| !s.is_empty())
    else {
        return Err(ApiError::Forbidden(
            "webhooks are disabled: set webhook_secret".into(),
        ));
    };
    if secret.len() < MIN_WEBHOOK_SECRET {
        return Err(ApiError::Forbidden(format!(
            "webhooks are disabled: webhook_secret must be at least {MIN_WEBHOOK_SECRET} characters"
        )));
    }
    // wrong tokens are counted per address (guessing is locked out)
    let key = format!("webhook:{}", reel_auth::ip_bucket(ip.as_deref()));
    st.auth
        .limiter()
        .reserve(&key, 10)
        .map_err(ApiError::Locked)?;
    let presented = presented_tokens(&headers, q.token.as_deref());
    if !presented
        .iter()
        .fold(false, |ok, t| secrets_match(t, secret) | ok)
    {
        return Err(ApiError::Forbidden("invalid webhook token".into()));
    }
    if q.token.as_deref().is_some_and(|t| secrets_match(t, secret)) {
        tracing::warn!(
            "webhook authenticated with ?token=; send it as the X-Reel-Token header instead"
        );
    }
    st.auth.limiter().refund(&key, 10);
    let outcome = st.requests.handle_webhook(&body, q.server).await?;

    let mut scanned = Vec::new();
    let mut ingested = false;
    for p in outcome
        .imported_paths
        .iter()
        .chain(outcome.removed_paths.iter())
    {
        let Some(path) = resolve_path(p) else {
            tracing::warn!("webhook path rejected (relative or containing `..`)");
            continue;
        };
        match st.scanner.scan_path(&st.db, &path).await {
            Ok(s) => {
                ingested |= s.scanned > 0;
                scanned.push(p.clone());
                if outcome.imported_paths.contains(p) {
                    st.schedule_import_scans(path.clone()).await;
                }
            }
            Err(reel_core::Error::NotFound) => {
                tracing::warn!(path = %p, "webhook path is not inside any library");
            }
            Err(e) => tracing::warn!(path = %p, "webhook rescan failed: {e}"),
        }
    }
    if let (true, Some(tmdb)) = (ingested, outcome.tmdb_id) {
        st.requests.mark_available_by_tmdb(tmdb).await?;
    }
    Ok(Json(WebhookResult {
        event: format!("{:?}", outcome.event),
        request_ids: outcome.request_ids,
        scanned_paths: scanned,
    }))
}

// ───────────────────────── admin: arr servers ─────────────────────────

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArrServerDto {
    pub id: i64,
    /// `sonarr` | `radarr`
    pub kind: String,
    pub name: String,
    pub url: String,
    /// Masked key (`••••` + last 4 characters); the key itself is never returned.
    pub api_key_hint: String,
    pub default_profile: Option<i64>,
    pub default_root: Option<String>,
    pub is_4k: bool,
}

fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n <= 4 {
        "••••".into()
    } else {
        format!("••••{}", s.chars().skip(n - 4).collect::<String>())
    }
}

fn arr_dto(r: &sqlx::sqlite::SqliteRow) -> ArrServerDto {
    ArrServerDto {
        id: r.get("id"),
        kind: r.get("kind"),
        name: r.get("name"),
        url: r.get("url"),
        api_key_hint: mask(&r.get::<String, _>("api_key")),
        default_profile: r.get("default_profile"),
        default_root: r.get("default_root"),
        is_4k: r.get::<i64, _>("is_4k") != 0,
    }
}

const ARR_SELECT: &str =
    "SELECT id, kind, name, url, api_key, default_profile, default_root, is_4k FROM arr_servers";

/// Verify URL + key by calling the instance's `system/status`.
async fn validate_arr(url: &str, key: &str, kind: &str) -> ApiResult<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::BadRequest(
            "url must start with http:// or https://".into(),
        ));
    }
    let status: Value = ArrClient::new(url, key)
        .get("/api/v3/system/status", &[])
        .await
        .map_err(|e| match e {
            reel_requests::Error::Remote {
                status: 401 | 403, ..
            } => ApiError::BadRequest("the server rejected the API key".into()),
            reel_requests::Error::Remote { status, .. } => {
                ApiError::BadRequest(format!("the server answered with HTTP {status}"))
            }
            e => ApiError::BadRequest(format!("cannot reach the server: {e}")),
        })?;
    // Radarr/Sonarr report their flavour in `appName`.
    if let Some(app) = status.get("appName").and_then(Value::as_str) {
        if !app.eq_ignore_ascii_case(kind) {
            return Err(ApiError::BadRequest(format!(
                "this is a {app} instance but kind is {kind}"
            )));
        }
    }
    Ok(())
}

/// List Radarr/Sonarr servers (API keys masked).
#[utoipa::path(get, path = "/admin/arr-servers", responses((status = 200, body = Vec<ArrServerDto>)))]
pub async fn list_arr_servers(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<ArrServerDto>>> {
    let rows = sqlx::query(&format!("{ARR_SELECT} ORDER BY id"))
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.iter().map(arr_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateArrServer {
    /// `sonarr` | `radarr`
    pub kind: String,
    pub name: String,
    pub url: String,
    pub api_key: String,
    pub default_profile: Option<i64>,
    pub default_root: Option<String>,
    #[serde(default)]
    pub is_4k: bool,
}

/// Add a server; URL and key are validated against `system/status` first.
#[utoipa::path(
    post,
    path = "/admin/arr-servers",
    request_body = CreateArrServer,
    responses((status = 201, body = ArrServerDto), (status = 400, body = ErrorBody))
)]
pub async fn create_arr_server(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<CreateArrServer>,
) -> ApiResult<(StatusCode, Json<ArrServerDto>)> {
    if !matches!(b.kind.as_str(), "radarr" | "sonarr") {
        return Err(ApiError::BadRequest("kind must be radarr or sonarr".into()));
    }
    if b.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let url = b.url.trim().trim_end_matches('/').to_string();
    validate_arr(&url, &b.api_key, &b.kind).await?;
    let id = sqlx::query(
        "INSERT INTO arr_servers (kind, name, url, api_key, default_profile, default_root, is_4k) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&b.kind)
    .bind(b.name.trim())
    .bind(&url)
    .bind(&b.api_key)
    .bind(b.default_profile)
    .bind(&b.default_root)
    .bind(i64::from(b.is_4k))
    .execute(&st.db)
    .await?
    .last_insert_rowid();
    st.audit(
        &a.user,
        "arr_server_create",
        Some(id.to_string()),
        json!({"name": b.name, "kind": b.kind}),
    )
    .await;
    let row = sqlx::query(&format!("{ARR_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok((StatusCode::CREATED, Json(arr_dto(&row))))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateArrServer {
    pub name: Option<String>,
    pub url: Option<String>,
    /// Omit to keep the current key.
    pub api_key: Option<String>,
    pub default_profile: Option<i64>,
    pub default_root: Option<String>,
    pub is_4k: Option<bool>,
}

/// Update a server (only the provided fields); re-validated when URL or key change.
#[utoipa::path(
    patch,
    path = "/admin/arr-servers/{id}",
    params(("id" = i64, Path, description = "Arr server id")),
    request_body = UpdateArrServer,
    responses((status = 200, body = ArrServerDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_arr_server(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<UpdateArrServer>,
) -> ApiResult<Json<ArrServerDto>> {
    let cur = sqlx::query(&format!("{ARR_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    let kind: String = cur.get("kind");
    let url = b
        .url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| cur.get("url"));
    let key = b.api_key.clone().unwrap_or_else(|| cur.get("api_key"));
    if b.url.is_some() || b.api_key.is_some() {
        validate_arr(&url, &key, &kind).await?;
    }
    let name = b
        .name
        .as_deref()
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| cur.get("name"));
    let is_4k = b.is_4k.unwrap_or_else(|| cur.get::<i64, _>("is_4k") != 0);
    sqlx::query(
        "UPDATE arr_servers SET name = ?, url = ?, api_key = ?, default_profile = ?, \
         default_root = ?, is_4k = ? WHERE id = ?",
    )
    .bind(name)
    .bind(&url)
    .bind(&key)
    .bind(b.default_profile.or_else(|| cur.get("default_profile")))
    .bind(b.default_root.or_else(|| cur.get("default_root")))
    .bind(i64::from(is_4k))
    .bind(id)
    .execute(&st.db)
    .await?;
    st.audit(
        &a.user,
        "arr_server_update",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    let row = sqlx::query(&format!("{ARR_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok(Json(arr_dto(&row)))
}

/// Remove a server (requests that used it keep their history).
#[utoipa::path(
    delete,
    path = "/admin/arr-servers/{id}",
    params(("id" = i64, Path, description = "Arr server id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody))
)]
pub async fn delete_arr_server(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let r = delete_row(&st, "arr_servers", id).await?;
    st.audit(
        &a.user,
        "arr_server_delete",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(r)
}

async fn delete_row(st: &AppState, table: &str, id: i64) -> ApiResult<StatusCode> {
    // `table` is always a literal from this module.
    let n = sqlx::query(&format!("DELETE FROM {table} WHERE id = ?"))
        .bind(id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if n > 0 {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QualityProfileDto {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RootFolderDto {
    pub id: i64,
    pub path: String,
    pub free_space: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArrProfiles {
    pub quality_profiles: Vec<QualityProfileDto>,
    pub root_folders: Vec<RootFolderDto>,
}

/// Quality profiles and root folders of a server, for the settings UI.
#[utoipa::path(
    get,
    path = "/admin/arr-servers/{id}/profiles",
    params(("id" = i64, Path, description = "Arr server id")),
    responses((status = 200, body = ArrProfiles), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody))
)]
pub async fn arr_profiles(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ArrProfiles>> {
    let row = sqlx::query("SELECT url, api_key FROM arr_servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    let c = ArrClient::new(row.get::<String, _>("url"), row.get::<String, _>("api_key"));
    let bad = |e: reel_requests::Error| ApiError::BadGateway(format!("server error: {e}"));
    let (profiles, roots) = (
        c.quality_profiles().await.map_err(bad)?,
        c.root_folders().await.map_err(bad)?,
    );
    Ok(Json(ArrProfiles {
        quality_profiles: profiles
            .into_iter()
            .map(|p| QualityProfileDto {
                id: p.id,
                name: p.name,
            })
            .collect(),
        root_folders: roots
            .into_iter()
            .map(|r| RootFolderDto {
                id: r.id,
                path: r.path,
                free_space: r.free_space,
            })
            .collect(),
    }))
}

// ───────────────────────── admin: download clients ─────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadClientDto {
    pub id: i64,
    /// `qbittorrent` | `transmission`
    pub kind: String,
    pub url: String,
    pub username: Option<String>,
    pub password_set: bool,
}

fn client_dto(r: &sqlx::sqlite::SqliteRow) -> DownloadClientDto {
    DownloadClientDto {
        id: r.get("id"),
        kind: r.get("kind"),
        url: r.get("url"),
        username: r.get("username"),
        password_set: r
            .get::<Option<String>, _>("password")
            .is_some_and(|p| !p.is_empty()),
    }
}

const CLIENT_SELECT: &str = "SELECT id, kind, url, username, password FROM download_clients";

/// List torrent clients (passwords never returned).
#[utoipa::path(get, path = "/admin/download-clients", responses((status = 200, body = Vec<DownloadClientDto>)))]
pub async fn list_download_clients(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<DownloadClientDto>>> {
    let rows = sqlx::query(&format!("{CLIENT_SELECT} ORDER BY id"))
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.iter().map(client_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDownloadClient {
    /// `qbittorrent` | `transmission`
    pub kind: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Add a torrent client (used for finer progress data: speed, ETA, seeds/peers).
#[utoipa::path(
    post,
    path = "/admin/download-clients",
    request_body = CreateDownloadClient,
    responses((status = 201, body = DownloadClientDto), (status = 400, body = ErrorBody))
)]
pub async fn create_download_client(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<CreateDownloadClient>,
) -> ApiResult<(StatusCode, Json<DownloadClientDto>)> {
    if !matches!(b.kind.as_str(), "qbittorrent" | "transmission") {
        return Err(ApiError::BadRequest(
            "kind must be qbittorrent or transmission".into(),
        ));
    }
    if !(b.url.starts_with("http://") || b.url.starts_with("https://")) {
        return Err(ApiError::BadRequest(
            "url must start with http:// or https://".into(),
        ));
    }
    let id = sqlx::query(
        "INSERT INTO download_clients (kind, url, username, password) VALUES (?, ?, ?, ?)",
    )
    .bind(&b.kind)
    .bind(b.url.trim().trim_end_matches('/'))
    .bind(&b.username)
    .bind(&b.password)
    .execute(&st.db)
    .await?
    .last_insert_rowid();
    st.audit(
        &a.user,
        "download_client_create",
        Some(id.to_string()),
        json!({"kind": b.kind}),
    )
    .await;
    let row = sqlx::query(&format!("{CLIENT_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok((StatusCode::CREATED, Json(client_dto(&row))))
}

/// Remove a torrent client.
#[utoipa::path(
    delete,
    path = "/admin/download-clients/{id}",
    params(("id" = i64, Path, description = "Download client id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody))
)]
pub async fn delete_download_client(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let r = delete_row(&st, "download_clients", id).await?;
    st.audit(
        &a.user,
        "download_client_delete",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(r)
}

// ───────────────────────── admin: notifications ─────────────────────────

const NOTIFY_EVENTS: &[&str] = &[
    "requested",
    "approved",
    "declined",
    "available",
    "failed",
    "issue",
];

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationDto {
    pub id: i64,
    /// `discord` | `telegram` | `webhook`
    pub kind: String,
    /// Target configuration with secrets masked.
    #[schema(value_type = Object)]
    pub config: Value,
    /// Events delivered to this target; empty = all.
    pub events: Vec<String>,
}

fn masked_config(v: &Value) -> Value {
    let Value::Object(m) = v else {
        return v.clone();
    };
    Value::Object(
        m.iter()
            .map(|(k, v)| {
                let secret = ["token", "secret", "password", "key", "url", "headers"]
                    .iter()
                    .any(|s| k.to_lowercase().contains(s));
                let v = match v {
                    Value::String(s) if secret => Value::String(mask(s)),
                    Value::Object(o) if secret => Value::Object(
                        o.iter()
                            .map(|(k, v)| {
                                (k.clone(), Value::String(mask(v.as_str().unwrap_or(""))))
                            })
                            .collect(),
                    ),
                    other => other.clone(),
                };
                (k.clone(), v)
            })
            .collect(),
    )
}

fn notification_dto(r: &sqlx::sqlite::SqliteRow) -> NotificationDto {
    let cfg: Value =
        serde_json::from_str(&r.get::<String, _>("config_json")).unwrap_or(Value::Null);
    NotificationDto {
        id: r.get("id"),
        kind: r.get("kind"),
        config: masked_config(&cfg),
        events: serde_json::from_str(&r.get::<String, _>("events_json")).unwrap_or_default(),
    }
}

const NOTIFY_SELECT: &str = "SELECT id, kind, config_json, events_json FROM notification_targets";

/// List notification targets (secrets masked).
#[utoipa::path(get, path = "/admin/notifications", responses((status = 200, body = Vec<NotificationDto>)))]
pub async fn list_notifications(
    State(st): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<NotificationDto>>> {
    let rows = sqlx::query(&format!("{NOTIFY_SELECT} ORDER BY id"))
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.iter().map(notification_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateNotification {
    /// `discord` (`webhook_url`), `telegram` (`bot_token`, `chat_id`) or `webhook` (`url`, `headers?`).
    pub kind: String,
    #[schema(value_type = Object)]
    pub config: Value,
    /// Subset of requested, approved, declined, available, failed, issue; empty = all.
    #[serde(default)]
    pub events: Vec<String>,
}

/// Add a notification target.
#[utoipa::path(
    post,
    path = "/admin/notifications",
    request_body = CreateNotification,
    responses((status = 201, body = NotificationDto), (status = 400, body = ErrorBody))
)]
pub async fn create_notification(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(b): Json<CreateNotification>,
) -> ApiResult<(StatusCode, Json<NotificationDto>)> {
    let required: &[&str] = match b.kind.as_str() {
        "discord" => &["webhook_url"],
        "telegram" => &["bot_token", "chat_id"],
        "webhook" => &["url"],
        _ => {
            return Err(ApiError::BadRequest(
                "kind must be discord, telegram or webhook".into(),
            ));
        }
    };
    if !b.config.is_object() {
        return Err(ApiError::BadRequest("config must be an object".into()));
    }
    for f in required {
        if b.config
            .get(*f)
            .is_none_or(|v| v.is_null() || v.as_str() == Some(""))
        {
            return Err(ApiError::BadRequest(format!("config.{f} is required")));
        }
    }
    if let Some(bad) = b
        .events
        .iter()
        .find(|e| !NOTIFY_EVENTS.contains(&e.as_str()))
    {
        return Err(ApiError::BadRequest(format!(
            "unknown event '{bad}' (available: {})",
            NOTIFY_EVENTS.join(", ")
        )));
    }
    let id = sqlx::query(
        "INSERT INTO notification_targets (kind, config_json, events_json) VALUES (?, ?, ?)",
    )
    .bind(&b.kind)
    .bind(b.config.to_string())
    .bind(json!(b.events).to_string())
    .execute(&st.db)
    .await?
    .last_insert_rowid();
    st.audit(
        &a.user,
        "notification_create",
        Some(id.to_string()),
        json!({"kind": b.kind}),
    )
    .await;
    let row = sqlx::query(&format!("{NOTIFY_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok((StatusCode::CREATED, Json(notification_dto(&row))))
}

/// Remove a notification target.
#[utoipa::path(
    delete,
    path = "/admin/notifications/{id}",
    params(("id" = i64, Path, description = "Notification target id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorBody))
)]
pub async fn delete_notification(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let r = delete_row(&st, "notification_targets", id).await?;
    st.audit(
        &a.user,
        "notification_delete",
        Some(id.to_string()),
        Value::Null,
    )
    .await;
    Ok(r)
}

/// Send a test message to one target (single attempt).
#[utoipa::path(
    post,
    path = "/admin/notifications/{id}/test",
    params(("id" = i64, Path, description = "Notification target id")),
    responses((status = 204, description = "Delivered"), (status = 404, body = ErrorBody), (status = 502, description = "Delivery failed", body = ErrorBody))
)]
pub async fn test_notification(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let t: Target = sqlx::query_as(&format!("{NOTIFY_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    let n = Notification {
        event: NotifyKind::Issue,
        request_id: None,
        title: "Reel test notification".into(),
        media_type: None,
        tmdb_id: None,
        user: Some("admin".into()),
        message: Some("If you can read this, notifications work.".into()),
    };
    st.requests
        .notifier()
        .deliver(&t, &n)
        .await
        .map_err(|e| ApiError::BadGateway(format!("delivery failed: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

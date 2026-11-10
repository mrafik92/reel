//! Xtream VOD (movies + series): catalog browsing, details (lazy provider info + TMDB), lanes,
//! playback through the normal decision engine, and a proxied direct-play stream.
//! The provider URL (it carries the credentials) never leaves the server.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reel_playback::{
    DecisionInput, DeviceProfile, PlaybackMode,
    decision::is_image_subtitle,
    model::{self, FileInfo, StreamKind},
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    auth::{AuthUser, SignedQuery, vis_clause, vis_json},
    browse::{Bind, LaneKind, bind_all, fold, text_score},
    dto::{AudioStream, CastMember},
    error::{ApiError, ApiResult, ErrorBody},
    playback::{DecideResponse, SubtitleOption},
    requests::tmdb_image,
};

const IMG_TTL: i64 = 2 * 86_400;
const IMG_QUANTUM: i64 = 86_400;
const DEFAULT_LIMIT: i64 = 40;
const MAX_LIMIT: i64 = 100;
const LANE_ITEMS: usize = 20;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ───────────────────────── runtime state ─────────────────────────

/// A running VOD playback (kept server-side: it knows the provider URL).
#[derive(Clone)]
pub struct VodSession {
    pub kind: &'static str,
    pub item_id: i64,
    pub user_id: i64,
    pub source_id: i64,
    /// Provider URL with credentials — never serialized.
    pub url: String,
    pub container: String,
    pub duration_s: Option<f64>,
    /// The source's configured User-Agent (`None`: the default one).
    pub user_agent: Option<String>,
}

impl VodSession {
    fn ua(&self) -> &str {
        self.user_agent
            .as_deref()
            .filter(|ua| !ua.trim().is_empty())
            .unwrap_or(reel_iptv::net::DEFAULT_UA)
    }
}

/// A running VOD playback plus its stop signal: dropping the sender (the session is forgotten,
/// stopped, taken over or reaped) ends every in-flight direct-play response, so a paused or
/// abandoned client cannot keep holding the provider's connection.
struct VodEntry {
    session: VodSession,
    stop: tokio::sync::watch::Sender<()>,
}

#[derive(Default)]
pub struct VodRuntime {
    sessions: Mutex<HashMap<String, VodEntry>>,
}

impl VodRuntime {
    pub fn get(&self, id: &str) -> Option<VodSession> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|e| e.session.clone())
    }
    /// The session plus a receiver whose `changed()` fails once the session is gone.
    fn subscribe(&self, id: &str) -> Option<(VodSession, tokio::sync::watch::Receiver<()>)> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|e| (e.session.clone(), e.stop.subscribe()))
    }
    pub fn insert(&self, id: &str, s: VodSession) {
        let (stop, _) = tokio::sync::watch::channel(());
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), VodEntry { session: s, stop });
    }
    pub fn forget(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }
    fn retain(&self, alive: &HashSet<String>) {
        self.sessions
            .lock()
            .unwrap()
            .retain(|k, _| alive.contains(k));
    }
}

impl AppState {
    /// Release provider slots of VOD playbacks whose playback session is gone (idle-reaped).
    pub async fn vod_reap(&self) -> usize {
        let alive: HashSet<String> = self
            .playback
            .list()
            .await
            .into_iter()
            .map(|s| s.id)
            .collect();
        self.vod.retain(&alive);
        self.iptv.live.vod_reap(&alive)
    }

    /// Background task: [`Self::vod_reap`] every few seconds.
    pub fn spawn_vod_reaper(&self) {
        let st = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                st.vod_reap().await;
            }
        });
    }
}

// ───────────────────────── DTOs ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodCategory {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodCard {
    pub id: i64,
    /// `movie` | `series`
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    /// 0–10
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    pub category_id: Option<i64>,
    /// The same title in your local library (movie, or show).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_item_id: Option<i64>,
    /// Watch progress 0..1 (movies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodPage {
    pub items: Vec<VodCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodMovieDetail {
    #[serde(flatten)]
    pub card: VodCard,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_s: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub director: Option<String>,
    pub cast: Vec<CastMember>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_youtube_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodEpisode {
    pub id: i64,
    pub episode_no: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub still_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_s: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodSeason {
    pub season_no: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    pub episodes: Vec<VodEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodSeriesDetail {
    #[serde(flatten)]
    pub card: VodCard,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_url: Option<String>,
    pub cast: Vec<CastMember>,
    pub seasons: Vec<VodSeason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodSeeAll {
    /// `movie` | `series`
    #[serde(rename = "type")]
    pub kind: String,
    /// Query string for `GET /vod/movies` / `GET /vod/series`.
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VodLane {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// `new` (recently added), `top_unwatched` (top rated) or `genre` (a category).
    pub kind: LaneKind,
    pub items: Vec<VodCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub see_all: Option<VodSeeAll>,
}

// ───────────────────────── helpers ─────────────────────────

fn vod_image(sg: &reel_auth::UrlSigner, what: &str, id: i64) -> String {
    sg.url(
        &format!("/api/v1/vod/images/{what}/{id}"),
        &format!("vodimg:{what}:{id}"),
        IMG_TTL,
        IMG_QUANTUM,
    )
}

fn parse_kind(s: Option<&str>) -> ApiResult<&'static str> {
    match s.unwrap_or("movie") {
        "movie" | "movies" => Ok("movie"),
        "series" | "tv" => Ok("series"),
        o => Err(ApiError::BadRequest(format!(
            "unknown type '{o}' (movie | series)"
        ))),
    }
}

fn split_genres(g: Option<String>) -> Option<Vec<String>> {
    g.map(|g| {
        g.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .filter(|v| !v.is_empty())
}

fn movie_from(uid: i64) -> String {
    format!(
        "vod_movies m LEFT JOIN vod_watch w ON w.user_id = {uid} AND w.kind = 'movie' AND w.item_id = m.id"
    )
}

/// Columns shared by both kinds (+ watch columns for movies).
fn card_cols(kind: &str) -> &'static str {
    if kind == "movie" {
        "m.id, m.title, m.year, m.poster, m.rating, m.genres, m.tmdb_id, m.category_id, \
         w.position_s AS pos, CAST(COALESCE(w.duration_s, m.runtime_s) AS REAL) AS dur, COALESCE(w.played, 0) AS played"
    } else {
        "m.id, m.title, m.year, m.poster, m.rating, m.genres, m.tmdb_id, m.category_id, \
         NULL AS pos, NULL AS dur, 0 AS played"
    }
}

fn card_from_row(kind: &str, r: &SqliteRow, sg: &reel_auth::UrlSigner) -> VodCard {
    let id: i64 = r.get("id");
    let poster: Option<String> = r.get("poster");
    let pos: Option<f64> = r.get("pos");
    let dur: Option<f64> = r.get::<Option<f64>, _>("dur").filter(|d| *d > 0.0);
    let progress = match (pos, dur) {
        (Some(p), Some(d)) if p > 0.0 => Some((p / d).clamp(0.0, 1.0)),
        _ => None,
    };
    let played = r.get::<i64, _>("played") != 0;
    VodCard {
        id,
        kind: kind.to_string(),
        title: r.get("title"),
        year: r.get("year"),
        poster_url: poster.map(|_| vod_image(sg, &format!("{kind}_poster"), id)),
        rating: r.get::<Option<f64>, _>("rating").filter(|x| *x > 0.0),
        genres: split_genres(r.get("genres")),
        tmdb_id: r.get("tmdb_id"),
        category_id: r.get("category_id"),
        library_item_id: None,
        progress,
        played: (kind == "movie").then_some(played),
    }
}

/// Link cards to the same title in the user's local library (by TMDB id).
async fn attach_library(st: &AppState, u: &AuthUser, cards: &mut [VodCard]) -> ApiResult<()> {
    if cards.iter().all(|c| c.tmdb_id.is_none()) {
        return Ok(());
    }
    let vis = vis_json(&st.visible(&u.user).await?);
    for (kind, lib_kind) in [("movie", "movie"), ("series", "show")] {
        let ids: Vec<i64> = cards
            .iter()
            .filter(|c| c.kind == kind)
            .filter_map(|c| c.tmdb_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let rows: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT i.tmdb_id, MIN(i.id) FROM media_items i WHERE i.kind = ? \
             AND i.tmdb_id IN (SELECT value FROM json_each(?)) AND {} GROUP BY i.tmdb_id",
            vis_clause("i.library_id")
        ))
        .bind(lib_kind)
        .bind(serde_json::to_string(&ids).unwrap_or_default())
        .bind(&vis)
        .bind(&vis)
        .fetch_all(&st.db)
        .await?;
        let map: HashMap<i64, i64> = rows.into_iter().collect();
        for c in cards.iter_mut().filter(|c| c.kind == kind) {
            c.library_item_id = c.tmdb_id.and_then(|t| map.get(&t).copied());
        }
    }
    Ok(())
}

// ───────────────────────── categories ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct TypeQuery {
    /// `movie` (default) or `series`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// VOD categories with item counts.
#[utoipa::path(get, path = "/vod/categories", params(TypeQuery), responses((status = 200, body = Vec<VodCategory>), (status = 400, body = ErrorBody)))]
pub async fn vod_categories(
    State(st): State<AppState>,
    _u: AuthUser,
    Query(q): Query<TypeQuery>,
) -> ApiResult<Json<Vec<VodCategory>>> {
    let kind = parse_kind(q.kind.as_deref())?;
    let table = if kind == "movie" {
        "vod_movies"
    } else {
        "vod_series"
    };
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(&format!(
        "SELECT c.id, c.name, count(i.id) AS n FROM vod_categories c \
         JOIN {table} i ON i.category_id = c.id AND i.missing = 0 \
         WHERE c.kind = ? GROUP BY c.id ORDER BY n DESC, c.name"
    ))
    .bind(kind)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, count)| VodCategory { id, name, count })
            .collect(),
    ))
}

// ───────────────────────── lists ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct VodListQuery {
    /// Category id from `/vod/categories`.
    pub category: Option<i64>,
    /// Title search (case/diacritics-insensitive substring).
    pub q: Option<String>,
    /// Genre(s), comma separated; all must match (only known once details were loaded).
    pub genre: Option<String>,
    /// `added` (default), `rating`, `title` or `year`.
    pub sort: Option<String>,
    pub cursor: Option<String>,
    /// Page size (default 40, max 100).
    pub limit: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    k: serde_json::Value,
    id: i64,
}

async fn list(
    st: &AppState,
    u: &AuthUser,
    kind: &'static str,
    q: VodListQuery,
) -> ApiResult<VodPage> {
    let sg = st.signer(u.id());
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let (key, desc, text) = match q.sort.as_deref().unwrap_or("added") {
        "added" => ("COALESCE(m.added_at, 0)", true, false),
        "rating" => ("CAST(COALESCE(m.rating, 0) * 100 AS INTEGER)", true, false),
        "title" => ("m.title_norm", false, true),
        "year" => ("COALESCE(m.year, 0)", true, false),
        o => return Err(ApiError::BadRequest(format!("unknown sort '{o}'"))),
    };
    let cursor = match q.cursor.as_deref() {
        None | Some("") => None,
        Some(c) => Some(
            URL_SAFE_NO_PAD
                .decode(c)
                .ok()
                .and_then(|b| serde_json::from_slice::<Cursor>(&b).ok())
                .filter(|c| c.k.is_string() == text && (text || c.k.is_i64()))
                .ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?,
        ),
    };
    let mut binds: Vec<Bind> = vec![];
    let mut sql_where = String::from("m.missing = 0");
    if let Some(c) = q.category {
        sql_where.push_str(" AND m.category_id = ?");
        binds.push(Bind::I(c));
    }
    let needle = fold(q.q.as_deref().unwrap_or(""));
    if !needle.is_empty() {
        sql_where.push_str(" AND m.title_norm LIKE ? ESCAPE '\\'");
        binds.push(Bind::S(format!(
            "%{}%",
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )));
    }
    for g in q
        .genre
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        sql_where.push_str(" AND (',' || lower(COALESCE(m.genres, '')) || ',') LIKE ?");
        binds.push(Bind::S(format!("%,{},%", g.to_lowercase())));
    }
    let (cmp, dir) = if desc { ("<", "DESC") } else { (">", "ASC") };
    let from = if kind == "movie" {
        movie_from(u.id())
    } else {
        "vod_series m".to_string()
    };
    let mut sql = format!(
        "SELECT {}, {key} AS sort_key FROM {from} WHERE {sql_where}",
        card_cols(kind)
    );
    if cursor.is_some() {
        sql.push_str(&format!(" AND ({key} {cmp} ? OR ({key} = ? AND m.id > ?))"));
    }
    sql.push_str(&format!(" ORDER BY {key} {dir}, m.id ASC LIMIT ?"));
    let mut query = bind_all(sqlx::query(&sql), &binds);
    if let Some(c) = &cursor {
        query = if let Some(s) = c.k.as_str() {
            query.bind(s.to_string()).bind(s.to_string())
        } else {
            let n = c.k.as_i64().unwrap_or_default();
            query.bind(n).bind(n)
        };
        query = query.bind(c.id);
    }
    let rows = query.bind(limit + 1).fetch_all(&st.db).await?;
    let has_more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let next_cursor = if has_more {
        rows.last().map(|r| {
            let k = if text {
                serde_json::Value::String(r.get::<String, _>("sort_key"))
            } else {
                serde_json::Value::from(r.get::<i64, _>("sort_key"))
            };
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&Cursor { k, id: r.get("id") }).unwrap_or_default())
        })
    } else {
        None
    };
    let mut items: Vec<VodCard> = rows.iter().map(|r| card_from_row(kind, r, &sg)).collect();
    attach_library(st, u, &mut items).await?;
    Ok(VodPage { items, next_cursor })
}

/// Browse VOD movies.
#[utoipa::path(get, path = "/vod/movies", params(VodListQuery), responses((status = 200, body = VodPage), (status = 400, body = ErrorBody)))]
pub async fn vod_movies(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<VodListQuery>,
) -> ApiResult<Json<VodPage>> {
    Ok(Json(list(&st, &u, "movie", q).await?))
}

/// Browse VOD series.
#[utoipa::path(get, path = "/vod/series", params(VodListQuery), responses((status = 200, body = VodPage), (status = 400, body = ErrorBody)))]
pub async fn vod_series(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<VodListQuery>,
) -> ApiResult<Json<VodPage>> {
    Ok(Json(list(&st, &u, "series", q).await?))
}

// ───────────────────────── details ─────────────────────────

async fn one_card(st: &AppState, u: &AuthUser, kind: &'static str, id: i64) -> ApiResult<VodCard> {
    let from = if kind == "movie" {
        movie_from(u.id())
    } else {
        "vod_series m".to_string()
    };
    let row = sqlx::query(&format!(
        "SELECT {} FROM {from} WHERE m.id = ? AND m.missing = 0",
        card_cols(kind)
    ))
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let mut cards = vec![card_from_row(kind, &row, &st.signer(u.id()))];
    attach_library(st, u, &mut cards).await?;
    Ok(cards.remove(0))
}

fn cast_names(json: Option<String>) -> Vec<String> {
    json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
        .unwrap_or_default()
}

async fn tmdb_cast(
    st: &AppState,
    sg: &reel_auth::UrlSigner,
    tv: bool,
    tmdb: i64,
) -> Option<(Vec<CastMember>, Option<String>, Option<String>, Option<i64>)> {
    let t = st.tmdb.as_ref()?;
    let (credits, overview, backdrop, runtime, genres) = if tv {
        let d = t.tv_cached(tmdb).await.ok()?;
        (
            d.credits,
            d.overview,
            d.backdrop_path,
            d.episode_run_time.first().copied().map(|m| m * 60),
            d.genres,
        )
    } else {
        let d = t.movie_cached(tmdb).await.ok()?;
        (
            d.credits,
            d.overview,
            d.backdrop_path,
            d.runtime.filter(|m| *m > 0).map(|m| m * 60),
            d.genres,
        )
    };
    let _ = genres;
    let mut cast = credits.map(|c| c.cast).unwrap_or_default();
    cast.sort_by_key(|m| m.order);
    let cast = cast
        .into_iter()
        .take(20)
        .map(|m| CastMember {
            name: m.name,
            character: m.character.filter(|c| !c.is_empty()),
            photo_url: tmdb_image(sg, "profile", &m.profile_path, 185),
            person_tmdb_id: Some(m.id),
        })
        .collect();
    Some((
        cast,
        overview.filter(|o| !o.trim().is_empty()),
        tmdb_image(sg, "backdrop", &backdrop, 780),
        runtime,
    ))
}

/// A VOD movie (provider details are loaded on first view; TMDB adds cast with person links).
#[utoipa::path(get, path = "/vod/movies/{id}", params(("id" = i64, Path, description = "VOD movie id")), responses((status = 200, body = VodMovieDetail), (status = 404, body = ErrorBody)))]
pub async fn vod_movie(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<VodMovieDetail>> {
    one_card(&st, &u, "movie", id).await?;
    if let Err(e) = st.iptv.ingest.ensure_movie_info(id).await {
        tracing::debug!(
            "vod info for movie {id} unavailable: {}",
            reel_iptv::net::redact_text(&e.to_string())
        );
    }
    let card = one_card(&st, &u, "movie", id).await?;
    let row = sqlx::query(
        "SELECT overview, backdrop, runtime_s, director, cast_json, trailer, container, added_at \
         FROM vod_movies WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    let sg = st.signer(u.id());
    let mut overview: Option<String> = row.get("overview");
    let mut backdrop = row
        .get::<Option<String>, _>("backdrop")
        .map(|_| vod_image(&sg, "movie_backdrop", id));
    let mut runtime: Option<i64> = row.get("runtime_s");
    let mut cast: Vec<CastMember> = cast_names(row.get("cast_json"))
        .into_iter()
        .map(|name| CastMember {
            name,
            character: None,
            photo_url: None,
            person_tmdb_id: None,
        })
        .collect();
    if let Some(tmdb) = card.tmdb_id {
        if let Some((tc, to, tb, tr)) = tmdb_cast(&st, &sg, false, tmdb).await {
            if !tc.is_empty() {
                cast = tc;
            }
            overview = overview.filter(|o| !o.trim().is_empty()).or(to);
            backdrop = backdrop.or(tb);
            runtime = runtime.or(tr);
        }
    }
    Ok(Json(VodMovieDetail {
        card,
        overview,
        backdrop_url: backdrop,
        runtime_s: runtime,
        director: row.get("director"),
        cast,
        trailer_youtube_id: row.get("trailer"),
        container: row.get("container"),
        added_at: row.get("added_at"),
    }))
}

/// A VOD series with seasons and episodes (loaded from the provider on first view).
#[utoipa::path(get, path = "/vod/series/{id}", params(("id" = i64, Path, description = "VOD series id")), responses((status = 200, body = VodSeriesDetail), (status = 404, body = ErrorBody)))]
pub async fn vod_series_detail(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<VodSeriesDetail>> {
    one_card(&st, &u, "series", id).await?;
    if let Err(e) = st.iptv.ingest.ensure_series_info(id).await {
        tracing::debug!(
            "vod info for series {id} unavailable: {}",
            reel_iptv::net::redact_text(&e.to_string())
        );
    }
    let card = one_card(&st, &u, "series", id).await?;
    let row = sqlx::query("SELECT overview, backdrop, cast_json FROM vod_series WHERE id = ?")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    let sg = st.signer(u.id());
    let mut overview: Option<String> = row.get("overview");
    let mut backdrop = row
        .get::<Option<String>, _>("backdrop")
        .map(|_| vod_image(&sg, "series_backdrop", id));
    let mut cast: Vec<CastMember> = cast_names(row.get("cast_json"))
        .into_iter()
        .map(|name| CastMember {
            name,
            character: None,
            photo_url: None,
            person_tmdb_id: None,
        })
        .collect();
    if let Some(tmdb) = card.tmdb_id {
        if let Some((tc, to, tb, _)) = tmdb_cast(&st, &sg, true, tmdb).await {
            if !tc.is_empty() {
                cast = tc;
            }
            overview = overview.filter(|o| !o.trim().is_empty()).or(to);
            backdrop = backdrop.or(tb);
        }
    }
    let seasons_meta: Vec<(i64, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT rowid, season_no, name, poster FROM vod_seasons WHERE series_pk = ?",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let eps = sqlx::query(
        "SELECT e.id, e.season_no, e.episode_no, e.title, e.overview, e.still, e.runtime_s, \
         w.position_s, CAST(COALESCE(w.duration_s, e.runtime_s) AS REAL) AS dur, COALESCE(w.played, 0) AS played \
         FROM vod_episodes e LEFT JOIN vod_watch w ON w.user_id = ? AND w.kind = 'episode' AND w.item_id = e.id \
         WHERE e.series_pk = ? ORDER BY e.season_no, e.episode_no, e.id",
    )
    .bind(u.id())
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let mut by_season: std::collections::BTreeMap<i64, Vec<VodEpisode>> = Default::default();
    for r in &eps {
        let eid: i64 = r.get("id");
        let pos: Option<f64> = r.get("position_s");
        let dur: Option<f64> = r.get::<Option<f64>, _>("dur").filter(|d| *d > 0.0);
        by_season
            .entry(r.get("season_no"))
            .or_default()
            .push(VodEpisode {
                id: eid,
                episode_no: r.get("episode_no"),
                title: r.get("title"),
                overview: r.get("overview"),
                still_url: r
                    .get::<Option<String>, _>("still")
                    .map(|_| vod_image(&sg, "episode_still", eid)),
                runtime_s: r.get("runtime_s"),
                progress: match (pos, dur) {
                    (Some(p), Some(d)) if p > 0.0 => Some((p / d).clamp(0.0, 1.0)),
                    _ => None,
                },
                played: Some(r.get::<i64, _>("played") != 0),
            });
    }
    let seasons = by_season
        .into_iter()
        .map(|(no, episodes)| {
            let meta = seasons_meta.iter().find(|m| m.1 == no);
            VodSeason {
                season_no: no,
                name: meta.and_then(|m| m.2.clone()),
                poster_url: meta
                    .filter(|m| m.3.is_some())
                    .map(|m| vod_image(&sg, "season_poster", m.0)),
                episodes,
            }
        })
        .collect();
    Ok(Json(VodSeriesDetail {
        card,
        overview,
        backdrop_url: backdrop,
        cast,
        seasons,
    }))
}

// ───────────────────────── images ─────────────────────────

/// Provider artwork (poster, backdrop, season poster, episode still), fetched once and cached.
#[utoipa::path(
    get,
    path = "/vod/images/{what}/{id}",
    params(
        ("what" = String, Path, description = "movie_poster | movie_backdrop | series_poster | series_backdrop | season_poster | episode_still"),
        ("id" = i64, Path, description = "Item id"),
        SignedQuery
    ),
    responses((status = 200, description = "Image bytes"), (status = 404, body = ErrorBody))
)]
pub async fn vod_image_route(
    State(st): State<AppState>,
    Path((what, id)): Path<(String, i64)>,
    Query(sq): Query<SignedQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    st.media_user(&headers, &sq, &format!("vodimg:{what}:{id}"))
        .await?;
    let sql = match what.as_str() {
        "movie_poster" => "SELECT poster FROM vod_movies WHERE id = ?",
        "movie_backdrop" => "SELECT backdrop FROM vod_movies WHERE id = ?",
        "series_poster" => "SELECT poster FROM vod_series WHERE id = ?",
        "series_backdrop" => "SELECT backdrop FROM vod_series WHERE id = ?",
        "season_poster" => "SELECT poster FROM vod_seasons WHERE rowid = ?",
        "episode_still" => "SELECT still FROM vod_episodes WHERE id = ?",
        _ => return Err(ApiError::NotFound),
    };
    let url: String = sqlx::query_scalar::<_, Option<String>>(sql)
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .flatten()
        .ok_or(ApiError::NotFound)?;
    let (bytes, ct) = crate::iptv::fetch_image_cached(&st, &url, "vod-images").await?;
    let mut res = Body::from(bytes).into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    Ok(res)
}

// ───────────────────────── lanes ─────────────────────────

async fn lane_cards(
    st: &AppState,
    u: &AuthUser,
    kind: &'static str,
    extra_where: &str,
    order: &str,
    bind_cat: Option<i64>,
) -> ApiResult<Vec<VodCard>> {
    let from = if kind == "movie" {
        movie_from(u.id())
    } else {
        "vod_series m".to_string()
    };
    let mut sql = format!(
        "SELECT {} FROM {from} WHERE m.missing = 0 {extra_where} ORDER BY {order}, m.id LIMIT {LANE_ITEMS}",
        card_cols(kind)
    );
    if bind_cat.is_none() {
        sql = sql.replace("m.category_id = ?", "1 = 1");
    }
    let mut q = sqlx::query(&sql);
    if let Some(c) = bind_cat {
        q = q.bind(c);
    }
    let rows = q.fetch_all(&st.db).await?;
    let sg = st.signer(u.id());
    let mut cards: Vec<VodCard> = rows.iter().map(|r| card_from_row(kind, r, &sg)).collect();
    attach_library(st, u, &mut cards).await?;
    Ok(cards)
}

/// Lanes for a VOD tab: recently added, top rated (unwatched first), then the biggest categories.
#[utoipa::path(get, path = "/vod/lanes", params(TypeQuery), responses((status = 200, body = Vec<VodLane>), (status = 400, body = ErrorBody)))]
pub async fn vod_lanes(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<TypeQuery>,
) -> ApiResult<Json<Vec<VodLane>>> {
    let kind = parse_kind(q.kind.as_deref())?;
    let see = |query: &str| {
        Some(VodSeeAll {
            kind: kind.to_string(),
            query: query.to_string(),
        })
    };
    let mut lanes = vec![];
    let new = lane_cards(&st, &u, kind, "", "COALESCE(m.added_at, 0) DESC", None).await?;
    if new.len() >= 3 {
        lanes.push(VodLane {
            key: "new".into(),
            title: "Recently added".into(),
            subtitle: None,
            kind: LaneKind::New,
            items: new,
            see_all: see("sort=added"),
        });
    }
    let unwatched = if kind == "movie" {
        "AND COALESCE(w.played, 0) = 0 AND m.rating IS NOT NULL"
    } else {
        "AND m.rating IS NOT NULL"
    };
    let top = lane_cards(&st, &u, kind, unwatched, "COALESCE(m.rating, 0) DESC", None).await?;
    if top.len() >= 3 {
        lanes.push(VodLane {
            key: "top_rated".into(),
            title: "Top rated".into(),
            subtitle: (kind == "movie").then(|| "Movies you haven't watched yet".to_string()),
            kind: LaneKind::TopUnwatched,
            items: top,
            see_all: see("sort=rating"),
        });
    }
    let table = if kind == "movie" {
        "vod_movies"
    } else {
        "vod_series"
    };
    let cats: Vec<(i64, String, i64)> = sqlx::query_as(&format!(
        "SELECT c.id, c.name, count(i.id) AS n FROM vod_categories c \
         JOIN {table} i ON i.category_id = c.id AND i.missing = 0 \
         WHERE c.kind = ? GROUP BY c.id HAVING n >= 6 ORDER BY n DESC, c.name LIMIT 10"
    ))
    .bind(kind)
    .fetch_all(&st.db)
    .await?;
    for (id, name, n) in cats {
        let items = lane_cards(
            &st,
            &u,
            kind,
            "AND m.category_id = ?",
            "COALESCE(m.added_at, 0) DESC",
            Some(id),
        )
        .await?;
        if items.len() >= 3 {
            lanes.push(VodLane {
                key: format!("category_{id}"),
                title: name,
                subtitle: Some(format!("{n} titles")),
                kind: LaneKind::Genre,
                items,
                see_all: see(&format!("category={id}&sort=added")),
            });
        }
    }
    Ok(Json(lanes))
}

// ───────────────────────── search ─────────────────────────

/// VOD titles matching a (folded) search needle, best matches first.
pub(crate) async fn search_cards(
    st: &AppState,
    u: &AuthUser,
    needle: &str,
    limit: usize,
) -> ApiResult<Vec<VodCard>> {
    if needle.is_empty() {
        return Ok(vec![]);
    }
    let sg = st.signer(u.id());
    let like = format!(
        "%{}%",
        needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut scored: Vec<(u32, f64, VodCard)> = vec![];
    for kind in ["movie", "series"] {
        let from = if kind == "movie" {
            movie_from(u.id())
        } else {
            "vod_series m".to_string()
        };
        let rows = sqlx::query(&format!(
            "SELECT {}, m.title_norm AS norm FROM {from} WHERE m.missing = 0 \
             AND m.title_norm LIKE ? ESCAPE '\\' LIMIT 300",
            card_cols(kind)
        ))
        .bind(&like)
        .fetch_all(&st.db)
        .await?;
        for r in &rows {
            let norm: String = r.get("norm");
            let s = text_score(&norm, needle);
            if s > 0 {
                let c = card_from_row(kind, r, &sg);
                scored.push((s, c.rating.unwrap_or(0.0), c));
            }
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.2.title.cmp(&b.2.title))
    });
    let mut cards: Vec<VodCard> = scored.into_iter().take(limit).map(|s| s.2).collect();
    attach_library(st, u, &mut cards).await?;
    Ok(cards)
}

// ───────────────────────── playback ─────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct VodPlayRequest {
    /// `movie` | `episode`
    pub kind: String,
    /// VOD movie id or episode id.
    pub id: i64,
    pub profile: Option<DeviceProfile>,
    pub preset: Option<String>,
    pub audio_index: Option<i64>,
    pub subtitle_index: Option<i64>,
    pub start_s: Option<f64>,
    pub device_id: Option<String>,
    /// Admins only: if every provider connection is held by other users, stop their streams.
    pub takeover: Option<bool>,
    /// Chromecast/AirPlay receiver: default preset `chromecast`, absolute 8-hour URLs on
    /// `cast_base_url`.
    pub for_cast: Option<bool>,
    /// `auto` (default) | `original` | `1080p` | `720p` | `480p` (see `POST /playback/{file}/decide`).
    pub quality: Option<reel_playback::Quality>,
    /// Client bandwidth estimate (kbit/s); the cap is `min(this, your bitrate limit)`.
    pub max_bitrate_kbps: Option<u32>,
}

fn ffprobe_path(st: &AppState) -> String {
    match st.config.ffmpeg_path.as_deref() {
        Some(p) if p.ends_with("ffmpeg") => format!("{}ffprobe", &p[..p.len() - 6]),
        _ => "ffprobe".into(),
    }
}

/// Negative ids keep VOD "files" apart from the library's `media_files` ids.
fn vod_file_id(kind: &str, id: i64) -> i64 {
    if kind == "movie" {
        -(2 * id)
    } else {
        -(2 * id + 1)
    }
}

async fn probe_cached(
    st: &AppState,
    kind: &str,
    id: i64,
    target: &reel_iptv::vod::PlayTarget,
) -> ApiResult<(FileInfo, Vec<model::StreamInfo>)> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT probe_json FROM vod_probe WHERE kind = ? AND item_id = ?")
            .bind(kind)
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    if let Some(j) = row {
        if let Ok((mut f, s)) = serde_json::from_str::<(FileInfo, Vec<model::StreamInfo>)>(&j) {
            f.path = target.url.clone();
            f.id = vod_file_id(kind, id);
            return Ok((f, s));
        }
    }
    let ua = target
        .user_agent
        .clone()
        .unwrap_or_else(|| reel_iptv::net::DEFAULT_UA.to_string());
    // the provider URL never reaches the subprocess: ffprobe reads through the loopback proxy,
    // under the shared ffmpeg process budget
    let media = st.playback.media();
    let _slot = media
        .proc(Duration::from_secs(30))
        .await
        .map_err(ApiError::from)?;
    let (probe_input, _lease) = media
        .proxied_input(&target.url, &ua, Duration::from_secs(120))
        .map_err(ApiError::from)?;
    let pr = reel_scanner::probe::probe_url(
        &ffprobe_path(st),
        &probe_input,
        &ua,
        Some(&target.container),
        Duration::from_secs(40),
    )
    .await
    .map_err(|e| {
        ApiError::BadGateway(format!(
            "cannot read the stream from the provider: {}",
            reel_iptv::net::redact_text(&e.to_string())
        ))
    })?;
    let streams: Vec<model::StreamInfo> = pr
        .streams
        .iter()
        .map(|s| model::StreamInfo {
            id: s.idx,
            idx: s.idx,
            kind: match s.kind {
                reel_scanner::probe::StreamKind::Video => StreamKind::Video,
                reel_scanner::probe::StreamKind::Audio => StreamKind::Audio,
                reel_scanner::probe::StreamKind::Subtitle => StreamKind::Subtitle,
            },
            codec: s.codec.clone(),
            profile: s.profile.clone(),
            level: s.level,
            width: s.width,
            height: s.height,
            hdr: model::Hdr::parse(s.hdr.as_str()),
            bit_depth: s.bit_depth,
            channels: s.channels,
            language: s.language.clone(),
            title: s.title.clone(),
            default: s.default,
            forced: s.forced,
            external_path: None,
        })
        .collect();
    let file = FileInfo {
        id: vod_file_id(kind, id),
        path: target.url.clone(),
        container: pr
            .container
            .clone()
            .or_else(|| Some(target.container.clone())),
        duration_s: pr.duration_s,
        bitrate: pr.bitrate,
    };
    let mut stored = file.clone();
    stored.path = String::new(); // credentials never go to the database
    if let Ok(j) = serde_json::to_string(&(&stored, &streams)) {
        let _ = sqlx::query(
            "INSERT INTO vod_probe (kind, item_id, probe_json, probed_at) VALUES (?,?,?,?) \
             ON CONFLICT(kind, item_id) DO UPDATE SET probe_json = excluded.probe_json, probed_at = excluded.probed_at",
        )
        .bind(kind)
        .bind(id)
        .bind(j)
        .bind(now())
        .execute(&st.db)
        .await;
    }
    Ok((file, streams))
}

/// Start playing a VOD movie or episode. Same answer as `POST /playback/{file}/decide`; progress
/// and stop use `/playback/{session}/progress` and `DELETE /playback/{session}`. The stream counts
/// against the provider's connection limit like live TV (your older streams are stopped first;
/// other users' streams → 429, or 409 + `takeover` for admins).
#[utoipa::path(
    post,
    path = "/vod/play",
    request_body = VodPlayRequest,
    responses(
        (status = 200, body = DecideResponse),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Admins: provider connections held by other users (`takeover: true`)", body = ErrorBody),
        (status = 429, body = ErrorBody),
        (status = 502, body = ErrorBody)
    )
)]
pub async fn vod_play(
    State(st): State<AppState>,
    u: AuthUser,
    Json(req): Json<VodPlayRequest>,
) -> ApiResult<Json<DecideResponse>> {
    let kind: &'static str = match req.kind.as_str() {
        "movie" => "movie",
        "episode" => "episode",
        o => {
            return Err(ApiError::BadRequest(format!(
                "unknown kind '{o}' (movie | episode)"
            )));
        }
    };
    let cast = crate::prefs::cast_ctx(&st, req.for_cast.unwrap_or(false))?;
    let profile = match (req.profile.clone(), req.preset.as_deref()) {
        (Some(p), _) => crate::prefs::sanitize_profile(&u.user, p),
        (None, name) => {
            let name = name.unwrap_or(if cast.is_some() {
                "chromecast"
            } else {
                "web_chrome"
            });
            DeviceProfile::preset(name).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "unknown preset '{name}' (available: {})",
                    DeviceProfile::preset_names().join(", ")
                ))
            })?
        }
    };
    let target = reel_iptv::vod::resolve_target(&st.db, kind, req.id)
        .await
        .map_err(|e| match e {
            reel_iptv::Error::NotFound(_) => ApiError::NotFound,
            e => e.into(),
        })?;
    // re-deciding the same title on the same device replaces the previous playback
    crate::playback::replace_previous(&st, u.id(), &req.device_id, vod_file_id(kind, req.id)).await;
    let limits = &u.user.limits;
    let mine: Vec<_> = st
        .playback
        .list()
        .await
        .into_iter()
        .filter(|s| s.user_id == u.id())
        .collect();
    if let Some(max) = limits.max_streams {
        if mine.len() as i64 >= max {
            return Err(ApiError::TooManyRequests(format!(
                "You reached your limit of {max} concurrent stream(s); stop another stream first"
            )));
        }
    }
    // provider connection budget (shared with live TV)
    let takeover = u.is_admin() && req.takeover.unwrap_or(false);
    let release_cb: Arc<dyn Fn(Option<String>) + Send + Sync> = {
        let (pm, vod, db) = (st.playback.clone(), st.vod_handle(), st.db.clone());
        Arc::new(move |sid: Option<String>| {
            let (pm, vod, db) = (pm.clone(), vod.clone(), db.clone());
            if let Some(sid) = sid {
                tokio::spawn(async move {
                    pm.remove(&sid).await;
                    vod.forget(&sid);
                    let _ = sqlx::query("DELETE FROM playback_sessions WHERE session_key = ?")
                        .bind(&sid)
                        .execute(&db)
                        .await;
                });
            }
        })
    };
    let lease = match st
        .iptv
        .live
        .vod_claim(
            u.id(),
            target.source_id,
            target.max_connections,
            &target.title,
            takeover,
            release_cb,
        )
        .await
    {
        Ok(l) => l,
        Err(reel_iptv::Error::InUseByOthers {
            source_id,
            max,
            holders,
        }) => {
            return Err(crate::iptv::in_use_error(&st, &u, source_id, max, &holders).await);
        }
        Err(e) => return Err(e.into()),
    };
    let res = vod_play_inner(&st, &u, &req, kind, &target, &profile, &mine, &cast).await;
    match res {
        Ok((resp, vs)) => {
            st.iptv.live.vod_attach(&lease, &resp.session_id);
            st.vod.insert(&resp.session_id, vs);
            Ok(Json(resp))
        }
        Err(e) => {
            st.iptv.live.vod_drop(&lease.id);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn vod_play_inner(
    st: &AppState,
    u: &AuthUser,
    req: &VodPlayRequest,
    kind: &'static str,
    target: &reel_iptv::vod::PlayTarget,
    profile: &DeviceProfile,
    mine: &[reel_playback::SessionInfo],
    cast: &Option<crate::prefs::Cast>,
) -> ApiResult<(DecideResponse, VodSession)> {
    let (file, streams) = probe_cached(st, kind, req.id, target).await?;
    let limits = &u.user.limits;
    let vs = VodSession {
        kind,
        item_id: req.id,
        user_id: u.id(),
        source_id: target.source_id,
        url: target.url.clone(),
        container: target.container.clone(),
        duration_s: file.duration_s,
        user_agent: target.user_agent.clone(),
    };
    // automatic audio/subtitle choice (client indexes override)
    let prefs = crate::prefs::load(st, u.id()).await;
    let original = match (crate::prefs::wants_original(&prefs), st.tmdb.as_ref()) {
        (true, Some(t)) => match item_tmdb(st, kind, req.id).await {
            Some((tv, id)) => crate::prefs::original_language(t, tv, id).await,
            None => None,
        },
        _ => None,
    };
    let mut selected = reel_playback::select::select(
        &streams,
        &prefs,
        original.as_deref(),
        reel_playback::Pick::from_request(req.audio_index),
        reel_playback::Pick::from_request(req.subtitle_index),
        profile,
    );
    // a text track we cannot extract (single-connection provider) is not worth selecting
    if let Some(si) = selected.subtitle_index {
        let text = streams.iter().any(|s| {
            s.idx == si
                && s.kind == StreamKind::Subtitle
                && !is_image_subtitle(s.codec.as_deref().unwrap_or(""))
        });
        if text && !sub_available(st, &vs, si).await {
            selected.subtitle_index = None;
            selected
                .reason
                .push_str("; subtitle track not extractable on a single-connection provider");
        }
    }
    let qd = reel_playback::decide_quality(
        &DecisionInput {
            file: &file,
            streams: &streams,
            audio_index: selected.audio_index,
            subtitle_index: selected.subtitle_index,
            profile,
            max_bitrate_kbps: limits.max_bitrate_kbps.and_then(|v| u32::try_from(v).ok()),
        },
        &reel_playback::QualityRequest {
            quality: req.quality.unwrap_or_default(),
            max_bitrate_kbps: req.max_bitrate_kbps.filter(|v| *v > 0),
        },
    );
    let (decision, quality, bitrate_kbps, quality_options) =
        (qd.decision, qd.quality, qd.bitrate_kbps, qd.options);
    if let Some(max) = limits.max_transcodes {
        let transcoding = mine
            .iter()
            .filter(|s| {
                matches!(
                    s.mode,
                    PlaybackMode::Transcode | PlaybackMode::AudioTranscode
                )
            })
            .count() as i64;
        if matches!(
            decision.mode,
            PlaybackMode::Transcode | PlaybackMode::AudioTranscode
        ) && transcoding >= max
        {
            return Err(ApiError::TooManyRequests(format!(
                "You reached your limit of {max} concurrent transcoded stream(s); stop another stream first"
            )));
        }
    }
    let info = st
        .playback
        .create_session(reel_playback::StartRequest {
            user_id: u.id(),
            device_id: req.device_id.clone(),
            file: file.clone(),
            streams: streams.clone(),
            decision: decision.clone(),
            start_position_s: req.start_s.unwrap_or(0.0).max(0.0),
            user_max_streams: limits.max_streams.and_then(|v| usize::try_from(v).ok()),
            user_max_transcodes: limits.max_transcodes.and_then(|v| usize::try_from(v).ok()),
            user_agent: target.user_agent.clone(),
        })
        .await?;
    let sg = st.signer(u.id());
    let ttl = if cast.is_some() {
        crate::prefs::Cast::ttl(st)
    } else {
        st.media_ttl()
    };
    let absolute = |u: String| match cast {
        Some(c) => c.abs(&u),
        None => u,
    };
    let url = absolute(if decision.mode == PlaybackMode::DirectPlay {
        sg.url(
            &format!("/api/v1/vod/direct/{}", info.id),
            &format!("vod:{}", info.id),
            ttl,
            1,
        )
    } else {
        sg.url(
            &format!("/api/v1/stream/{}/master.m3u8", info.id),
            &format!("stream:{}", info.id),
            ttl,
            1,
        )
    });
    let mut subtitles = vec![];
    for s in streams.iter().filter(|s| s.kind == StreamKind::Subtitle) {
        let text = !is_image_subtitle(s.codec.as_deref().unwrap_or(""));
        // text tracks are extracted from the provider file on demand (image subs: burn-in only)
        let url = if text && sub_available(st, &vs, s.idx).await {
            Some(absolute(sg.url(
                &format!("/api/v1/vod/subs/{}/{}.vtt", info.id, s.idx),
                &format!("vodsub:{}:{}", info.id, s.idx),
                ttl,
                1,
            )))
        } else {
            None
        };
        subtitles.push(SubtitleOption {
            index: s.idx,
            stream_id: s.id,
            language: s.language.clone(),
            title: s.title.clone(),
            url,
            forced: s.forced,
            external: false,
        });
    }
    let audio = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .map(|s| AudioStream {
            index: s.idx,
            codec: s.codec.clone().unwrap_or_default(),
            channels: s.channels.unwrap_or(0),
            language: s.language.clone(),
            title: s.title.clone(),
            default: s.default,
        })
        .collect();
    Ok((
        DecideResponse {
            session_id: info.id,
            mode: decision.mode,
            url,
            decision,
            subtitles,
            audio,
            selected,
            bitrate_kbps,
            quality,
            quality_options,
        },
        vs,
    ))
}

/// Persist progress of a VOD playback (`/playback/{session}/progress` for VOD sessions).
pub(crate) async fn record_progress(st: &AppState, vs: &VodSession, pos: f64) -> ApiResult<()> {
    let finished = vs.duration_s.is_some_and(|d| d > 0.0 && pos >= d * 0.9);
    sqlx::query(
        "INSERT INTO vod_watch (user_id, kind, item_id, position_s, duration_s, played, last_played_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(user_id, kind, item_id) DO UPDATE SET position_s = excluded.position_s, \
         duration_s = COALESCE(excluded.duration_s, duration_s), \
         played = MAX(played, excluded.played), last_played_at = excluded.last_played_at",
    )
    .bind(vs.user_id)
    .bind(vs.kind)
    .bind(vs.item_id)
    .bind(pos)
    .bind(vs.duration_s)
    .bind(i64::from(finished))
    .bind(now())
    .execute(&st.db)
    .await?;
    Ok(())
}

/// Direct play of a VOD stream: proxied through Reel with `Range` forwarded; the provider URL
/// and credentials stay on the server. Signed URL (scope `vod:{session}`) or bearer token.
#[utoipa::path(
    get,
    path = "/vod/direct/{session_id}",
    params(("session_id" = String, Path, description = "Session id from /vod/play"), SignedQuery),
    responses(
        (status = 200, description = "Media bytes"),
        (status = 206, description = "Partial content"),
        (status = 404, body = ErrorBody),
        (status = 502, body = ErrorBody)
    )
)]
pub async fn vod_direct(
    State(st): State<AppState>,
    Path(sid): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let sq = Query::<SignedQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();
    let user = st
        .media_user(req.headers(), &sq, &format!("vod:{sid}"))
        .await?;
    let (vs, stop) = st.vod.subscribe(&sid).ok_or(ApiError::NotFound)?;
    if vs.user_id != user.id && !user.is_admin() {
        return Err(ApiError::NotFound);
    }
    st.playback.touch_session(&sid);
    let mut up = st
        .iptv
        .ingest
        .http
        .get(&vs.url)
        .header(header::USER_AGENT, vs.ua())
        .header(header::ACCEPT, "*/*");
    if let Some(r) = req.headers().get(header::RANGE) {
        up = up.header(header::RANGE, r.clone());
    }
    let resp = up
        .send()
        .await
        .map_err(|e| ApiError::BadGateway(format!("provider unreachable: {}", e.without_url())))?;
    let code = resp.status().as_u16();
    let gate = st.iptv.gates.get(vs.source_id);
    if code == 429 || (460..470).contains(&code) {
        gate.report(code, None);
    }
    if !(resp.status().is_success() || code == 416) {
        return Err(ApiError::BadGateway(format!(
            "the provider answered HTTP {code}"
        )));
    }
    let mut out = Response::builder().status(resp.status());
    for h in [
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(v) = resp.headers().get(&h) {
            out = out.header(h, v.clone());
        }
    }
    out = out
        .header(
            header::CONTENT_TYPE,
            crate::routes::container_mime(&vs.container),
        )
        .header(header::CACHE_CONTROL, "no-store");
    if resp.headers().get(header::ACCEPT_RANGES).is_none() {
        out = out.header(header::ACCEPT_RANGES, "bytes");
    }
    // the provider connection lives exactly as long as the playback session: a stop, takeover or
    // idle reap ends the body and drops the upstream response. A task owns the upstream so this
    // also happens while the client is paused (no body polling under backpressure).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, reqwest::Error>>(4);
    tokio::spawn(pump_until_stopped(resp, tx, stop));
    let body =
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) });
    out.body(Body::from_stream(body))
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Copy `resp` into `tx` until it ends, the client goes away or `stop` fires (sender dropped).
async fn pump_until_stopped(
    resp: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<bytes::Bytes, reqwest::Error>>,
    mut stop: tokio::sync::watch::Receiver<()>,
) {
    use futures_util::StreamExt;
    let mut up = Box::pin(resp.bytes_stream());
    loop {
        let chunk = tokio::select! {
            biased;
            _ = stop.changed() => return,
            _ = tx.closed() => return,
            c = up.next() => match c {
                Some(c) => c,
                None => return,
            },
        };
        let failed = chunk.is_err();
        tokio::select! {
            biased;
            _ = stop.changed() => return,
            r = tx.send(chunk) => if r.is_err() || failed { return },
        }
    }
}

fn sub_cache_name(vs: &VodSession, idx: i64) -> String {
    format!("vod-{}-{}-{idx}", vs.kind, vs.item_id)
}

/// Streams of a probed VOD item (empty when not probed yet).
pub(crate) async fn cached_streams(st: &AppState, kind: &str, id: i64) -> Vec<model::StreamInfo> {
    let j: Option<String> =
        sqlx::query_scalar("SELECT probe_json FROM vod_probe WHERE kind = ? AND item_id = ?")
            .bind(kind)
            .bind(id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    j.and_then(|j| serde_json::from_str::<(FileInfo, Vec<model::StreamInfo>)>(&j).ok())
        .map(|x| x.1)
        .unwrap_or_default()
}

/// Can the text subtitle `idx` be served? Extraction reads the whole remote file over a *second*
/// provider connection, so it only runs when the provider allows more than one connection —
/// unless the track was extracted (cached) before.
pub(crate) async fn sub_available(st: &AppState, vs: &VodSession, idx: i64) -> bool {
    let cached = st
        .config
        .data_dir
        .join("subs")
        .join(format!("{}.vtt", sub_cache_name(vs, idx)));
    if tokio::fs::metadata(&cached)
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        return true;
    }
    can_extract_subs(st, vs).await
}

pub(crate) async fn can_extract_subs(st: &AppState, vs: &VodSession) -> bool {
    let max: Option<i64> =
        sqlx::query_scalar("SELECT max_connections FROM iptv_sources WHERE id = ?")
            .bind(vs.source_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    !matches!(max, Some(1))
}

/// TMDB `(is_tv, id)` of a VOD movie / episode.
async fn item_tmdb(st: &AppState, kind: &str, id: i64) -> Option<(bool, i64)> {
    let (sql, tv) = if kind == "movie" {
        ("SELECT tmdb_id FROM vod_movies WHERE id = ?", false)
    } else {
        (
            "SELECT s.tmdb_id FROM vod_episodes e JOIN vod_series s ON s.id = e.series_pk WHERE e.id = ?",
            true,
        )
    };
    let t: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(sql)
        .bind(id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
        .flatten();
    t.map(|t| (tv, t))
}

/// An embedded text subtitle of a VOD playback as WebVTT (extracted from the provider file with
/// the player User-Agent, cached). Signed URL (scope `vodsub:{session}:{index}`) or bearer token.
#[utoipa::path(
    get,
    path = "/vod/subs/{session_id}/{file}",
    params(
        ("session_id" = String, Path, description = "Session id from /vod/play"),
        ("file" = String, Path, description = "`{subtitle index}.vtt`"),
        SignedQuery
    ),
    responses(
        (status = 200, description = "WebVTT", content_type = "text/vtt"),
        (status = 404, body = ErrorBody),
        (status = 503, description = "Not available on single-connection providers", body = ErrorBody)
    )
)]
pub async fn vod_subs(
    State(st): State<AppState>,
    Path((sid, file)): Path<(String, String)>,
    Query(sq): Query<SignedQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let idx: i64 = file
        .strip_suffix(".vtt")
        .and_then(|n| n.parse().ok())
        .ok_or(ApiError::NotFound)?;
    let user = st
        .media_user(&headers, &sq, &format!("vodsub:{sid}:{idx}"))
        .await?;
    let vs = st.vod.get(&sid).ok_or(ApiError::NotFound)?;
    if vs.user_id != user.id && !user.is_admin() {
        return Err(ApiError::NotFound);
    }
    let streams = cached_streams(&st, vs.kind, vs.item_id).await;
    let is_text_sub = streams.iter().any(|s| {
        s.idx == idx
            && s.kind == StreamKind::Subtitle
            && !is_image_subtitle(s.codec.as_deref().unwrap_or(""))
    });
    if !is_text_sub {
        return Err(ApiError::NotFound);
    }
    if !sub_available(&st, &vs, idx).await {
        return Err(ApiError::Status(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "subtitles cannot be extracted while streaming from a single-connection provider"
                .into(),
        ));
    }
    st.playback.touch_session(&sid);
    let out = reel_playback::subtitles::to_vtt_with_ua(
        st.playback.media(),
        &st.playback.config().ffmpeg_path,
        &reel_playback::subtitles::SubtitleSource::Embedded {
            path: vs.url.clone(),
            stream_index: idx,
        },
        &st.config.data_dir.join("subs"),
        &sub_cache_name(&vs, idx),
        vs.user_agent.as_deref(),
    )
    .await
    .map_err(|e| ApiError::BadGateway(reel_iptv::net::redact_text(&e.to_string())))?;
    let bytes = tokio::fs::read(out)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut res = bytes.into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/vtt; charset=utf-8"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    Ok(res)
}

impl AppState {
    /// Shared handle to the VOD runtime (for callbacks).
    pub fn vod_handle(&self) -> Arc<VodRuntime> {
        self.vod.clone()
    }
}

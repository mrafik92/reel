//! Phase 1 REST routes (SPEC §4.1).

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reel_metadata::ImageKind;
use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use utoipa::IntoParams;

use crate::{
    AppState,
    auth::{AdminUser, AuthUser, SignedQuery, vis_clause, vis_json},
    dto::*,
    error::{ApiError, ApiResult, ErrorBody},
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const ROW_LIMIT: i64 = 20;

/// Image URL lifetime; signatures are quantized to a day so URLs stay cacheable.
const IMG_TTL: i64 = 2 * 86_400;
const IMG_QUANTUM: i64 = 86_400;

/// Columns + joins producing an [`ItemSummary`] (see [`summary_from_row`]) for user `uid`.
/// SQL expression: 1 if user `uid` has watched item `i` (every episode, for shows/seasons).
pub(crate) fn played_expr(uid: i64) -> String {
    format!(
        "CASE WHEN i.kind IN ('show','season') THEN COALESCE((SELECT count(*) > 0 AND \
        sum(CASE WHEN COALESCE(cw.played, 0) = 1 THEN 0 ELSE 1 END) = 0 \
        FROM media_items c LEFT JOIN watch_state cw ON cw.item_id = c.id AND cw.user_id = {uid} \
        WHERE c.kind = 'episode' AND (c.parent_id = i.id OR c.parent_id IN \
            (SELECT id FROM media_items WHERE parent_id = i.id))), 0) \
        ELSE COALESCE(w.played, 0) END"
    )
}

pub(crate) fn summary_select(uid: i64) -> String {
    format!(
        "SELECT i.id, i.kind, i.title, i.year, i.poster, i.backdrop, \
    i.season_no, i.episode_no, i.parent_id, i.added_at, w.position_s, i.rating AS s_rating, \
    {} AS played, \
    CAST(COALESCE(i.runtime_s, (SELECT f.duration_s FROM media_files f WHERE f.item_id = i.id \
        OR f.id IN (SELECT file_id FROM media_file_links WHERE item_id = i.id) \
        ORDER BY f.part_no, f.id LIMIT 1)) AS REAL) AS dur",
        played_expr(uid)
    )
}

pub(crate) fn summary_from(uid: i64) -> String {
    format!(
        " FROM media_items i LEFT JOIN watch_state w \
    ON w.item_id = i.id AND w.user_id = {uid}"
    )
}

/// Signed `/api/v1/images/{kind}/{id}?w=…` URL.
pub(crate) fn image_url(sg: &reel_auth::UrlSigner, kind: &str, id: i64, w: u32) -> String {
    sg.url(
        &format!("/api/v1/images/{kind}/{id}?w={w}"),
        &format!("img:{kind}:{id}"),
        IMG_TTL,
        IMG_QUANTUM,
    )
}

pub(crate) fn summary_from_row(r: &SqliteRow, sg: &reel_auth::UrlSigner) -> ItemSummary {
    let id: i64 = r.get("id");
    let poster: Option<String> = r.get("poster");
    let backdrop: Option<String> = r.get("backdrop");
    let pos: Option<f64> = r.get("position_s");
    let dur: Option<f64> = r.get("dur");
    let progress = match (pos, dur) {
        (Some(p), Some(d)) if p > 0.0 && d > 0.0 => Some((p / d).clamp(0.0, 1.0)),
        _ => None,
    };
    ItemSummary {
        id,
        kind: ItemKind::parse(&r.get::<String, _>("kind")),
        title: r.get("title"),
        year: r.get("year"),
        poster_url: poster.map(|_| image_url(sg, "poster", id, 342)),
        backdrop_url: backdrop.map(|_| image_url(sg, "backdrop", id, 780)),
        season_no: r.get("season_no"),
        episode_no: r.get("episode_no"),
        parent_id: r.get("parent_id"),
        progress,
        played: r.get::<i64, _>("played") != 0,
        added_at: r.get("added_at"),
        rating: None,
        runtime_s: None,
        genres: None,
        series: None,
    }
}

/// Summaries for list endpoints: adds `rating`, `runtime_s` and `genres` (one batched genre query
/// for the whole page).
pub(crate) async fn summaries(
    st: &AppState,
    rows: &[SqliteRow],
    sg: &reel_auth::UrlSigner,
) -> ApiResult<Vec<ItemSummary>> {
    let mut out: Vec<ItemSummary> = rows
        .iter()
        .map(|r| {
            let mut s = summary_from_row(r, sg);
            s.rating = r.get::<Option<f64>, _>("s_rating").filter(|x| *x > 0.0);
            s.runtime_s = r
                .get::<Option<f64>, _>("dur")
                .filter(|d| *d > 0.0)
                .map(|d| d.round() as i64);
            s
        })
        .collect();
    attach_genres(st, &mut out).await?;
    Ok(out)
}

pub(crate) async fn attach_genres(st: &AppState, items: &mut [ItemSummary]) -> ApiResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let ids = serde_json::to_string(&items.iter().map(|i| i.id).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into());
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT ig.item_id, g.name FROM item_genres ig JOIN genres g ON g.id = ig.genre_id \
         WHERE ig.item_id IN (SELECT value FROM json_each(?)) ORDER BY g.name",
    )
    .bind(ids)
    .fetch_all(&st.db)
    .await?;
    let mut by: std::collections::HashMap<i64, Vec<String>> = Default::default();
    for (id, g) in rows {
        by.entry(id).or_default().push(g);
    }
    for i in items {
        i.genres = Some(by.remove(&i.id).unwrap_or_default());
    }
    Ok(())
}

// ───────────────────────── libraries ─────────────────────────

fn library_from_row(r: &SqliteRow) -> Library {
    let paths: Vec<String> =
        serde_json::from_str(&r.get::<String, _>("paths_json")).unwrap_or_default();
    Library {
        id: r.get("id"),
        name: r.get("name"),
        kind: LibraryKind::parse(&r.get::<String, _>("kind")).unwrap_or(LibraryKind::Movies),
        paths,
        item_count: r.get("item_count"),
    }
}

const LIBRARY_SELECT: &str = "SELECT l.id, l.name, l.kind, l.paths_json, \
    (SELECT count(*) FROM media_items m WHERE m.library_id = l.id \
        AND m.kind IN ('movie','show')) AS item_count FROM libraries l";

/// List libraries.
#[utoipa::path(get, path = "/libraries", responses((status = 200, body = Vec<Library>)))]
pub async fn list_libraries(
    State(st): State<AppState>,
    u: AuthUser,
) -> ApiResult<Json<Vec<Library>>> {
    let vis = vis_json(&st.visible(&u.user).await?);
    let rows = sqlx::query(&format!(
        "{LIBRARY_SELECT} WHERE {} ORDER BY l.id",
        vis_clause("l.id")
    ))
    .bind(&vis)
    .bind(&vis)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.iter().map(library_from_row).collect()))
}

/// Create a library and start scanning it in the background.
#[utoipa::path(
    post,
    path = "/libraries",
    request_body = CreateLibrary,
    responses((status = 201, body = Library), (status = 400, body = ErrorBody))
)]
pub async fn create_library(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Json(body): Json<CreateLibrary>,
) -> ApiResult<(StatusCode, Json<Library>)> {
    let id = st
        .create_library(&body.name, body.kind, &body.paths)
        .await
        .map_err(ApiError::BadRequest)?;
    st.audit(
        &a.user,
        "library_create",
        Some(id.to_string()),
        serde_json::json!({"name": body.name}),
    )
    .await;
    let row = sqlx::query(&format!("{LIBRARY_SELECT} WHERE l.id = ?"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok((StatusCode::CREATED, Json(library_from_row(&row))))
}

/// Trigger a library scan (runs in the background).
#[utoipa::path(
    post,
    path = "/libraries/{id}/scan",
    params(("id" = i64, Path, description = "Library id")),
    responses((status = 202, description = "Scan started"), (status = 404, body = ErrorBody))
)]
pub async fn scan_library(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE id = ?")
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    st.spawn_scan(id);
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Deserialize, IntoParams)]
#[allow(dead_code)]
pub struct ItemsQuery {
    /// `title` (default, A→Z), `added` (newest first), `year` (newest first), `rating` (best
    /// first), `runtime` (shortest first) or `random` (stable for a given `seed`).
    pub sort: Option<String>,
    /// Opaque cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size (default 50, max 200).
    pub limit: Option<i64>,
    /// Genre name; repeat the parameter or use a comma list. All given genres must match.
    pub genre: Option<String>,
    /// Earliest release year.
    pub year_from: Option<i64>,
    /// Latest release year.
    pub year_to: Option<i64>,
    /// Minimum rating (0–10).
    pub min_rating: Option<f64>,
    /// Maximum runtime in minutes.
    pub max_runtime_min: Option<i64>,
    /// Only items the current user has not (fully) watched.
    pub unwatched: Option<bool>,
    /// Seed for `sort=random` (integer; same seed = same order, so pagination is stable).
    pub seed: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    k: serde_json::Value,
    id: i64,
}

/// Top-level items (movies or shows) of a library, keyset-paginated, with optional filters.
#[utoipa::path(
    get,
    path = "/libraries/{id}/items",
    params(("id" = i64, Path, description = "Library id"), ItemsQuery),
    responses((status = 200, body = ItemPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn library_items(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<ItemPage>> {
    let pairs = crate::browse::parse_pairs(raw.as_deref());
    let one = |k: &str| {
        pairs
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
    };
    let filters = crate::browse::Filters::from_pairs(&pairs)?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE id = ?")
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    st.require_library(&u.user, id).await?;
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let limit = one("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let seed: i64 = match one("seed") {
        Some(s) => s
            .parse()
            .map_err(|_| ApiError::BadRequest("seed must be an integer".into()))?,
        None => 0,
    };
    // multiplier per seed → a different permutation for every seed (mod a prime)
    let mult: i64 = ((seed.rem_euclid(1_000_003) + 1) * 2_654_435_761) % 4_294_967_291;
    // (key expression, descending?, text key?)
    let (key, desc, text) = match one("sort").as_deref().unwrap_or("title") {
        "title" => (
            "COALESCE(i.sort_title, lower(i.title))".to_string(),
            false,
            true,
        ),
        "added" => ("i.added_at".to_string(), true, false),
        "year" => ("COALESCE(i.year, 0)".to_string(), true, false),
        "rating" => (
            "CAST(COALESCE(i.rating, 0) * 100 AS INTEGER)".to_string(),
            true,
            false,
        ),
        "runtime" => (
            format!(
                "COALESCE(CAST({} AS INTEGER), 999999999)",
                crate::browse::RUNTIME_EXPR
            ),
            false,
            false,
        ),
        "random" => (format!("((i.id * {mult}) % 4294967291)"), false, false),
        other => return Err(ApiError::BadRequest(format!("unknown sort '{other}'"))),
    };
    let cursor = match one("cursor").as_deref() {
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
    let cmp = if desc { "<" } else { ">" };
    let dir = if desc { "DESC" } else { "ASC" };
    let (fsql, fbinds) = filters.sql(u.id());
    let mut sql = format!(
        "{sel}, {key} AS sort_key{frm} \
         WHERE i.library_id = ? AND i.kind IN ('movie','show'){fsql}"
    );
    if cursor.is_some() {
        sql.push_str(&format!(" AND ({key} {cmp} ? OR ({key} = ? AND i.id > ?))"));
    }
    sql.push_str(&format!(" ORDER BY {key} {dir}, i.id ASC LIMIT ?"));
    let mut query = sqlx::query(&sql).bind(id);
    query = crate::browse::bind_all(query, &fbinds);
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
            let c = Cursor { k, id: r.get("id") };
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).unwrap_or_default())
        })
    } else {
        None
    };
    Ok(Json(ItemPage {
        items: summaries(&st, rows, &sg).await?,
        next_cursor,
    }))
}

// ───────────────────────── items ─────────────────────────

/// Item detail with files, streams, cast and genres.
#[utoipa::path(
    get,
    path = "/items/{id}",
    params(("id" = i64, Path, description = "Item id")),
    responses((status = 200, body = ItemDetail), (status = 404, body = ErrorBody))
)]
pub async fn get_item(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ItemDetail>> {
    st.require_item(&u.user, id).await?;
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let row = sqlx::query(&format!(
        "{sel}, i.overview, i.runtime_s, i.rating, i.content_rating, i.tmdb_id, \
         (SELECT count(*) FROM media_items c WHERE c.parent_id = i.id) AS children_count \
         {frm} WHERE i.id = ?"
    ))
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let genres: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM item_genres ig JOIN genres g ON g.id = ig.genre_id \
         WHERE ig.item_id = ? ORDER BY g.name",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;

    let cast = sqlx::query(
        "SELECT p.id, p.name, p.photo, p.tmdb_id, c.character FROM credits c \
         JOIN people p ON p.id = c.person_id \
         WHERE c.item_id = ? AND c.role = 'actor' ORDER BY c.ord, p.id",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?
    .iter()
    .map(|r| {
        let photo: Option<String> = r.get("photo");
        CastMember {
            name: r.get("name"),
            character: r.get("character"),
            photo_url: photo.map(|_| image_url(&sg, "person", r.get("id"), 185)),
            person_tmdb_id: r.get("tmdb_id"),
        }
    })
    .collect();

    let files = load_files(&st, &sg, id).await?;
    Ok(Json(ItemDetail {
        summary: summary_from_row(&row, &sg),
        overview: row.get("overview"),
        runtime_s: row.get("runtime_s"),
        rating: row.get("rating"),
        content_rating: row.get("content_rating"),
        genres,
        tmdb_id: row.get("tmdb_id"),
        cast,
        children_count: row.get("children_count"),
        files,
    }))
}

async fn load_files(
    st: &AppState,
    sg: &reel_auth::UrlSigner,
    item_id: i64,
) -> ApiResult<Vec<MediaFile>> {
    let file_rows = sqlx::query(
        "SELECT id, container, duration_s, size FROM media_files WHERE item_id = ?1 \
         OR id IN (SELECT file_id FROM media_file_links WHERE item_id = ?1) \
         ORDER BY part_no, id",
    )
    .bind(item_id)
    .fetch_all(&st.db)
    .await?;
    let mut files = Vec::with_capacity(file_rows.len());
    for fr in &file_rows {
        let fid: i64 = fr.get("id");
        // Multi-episode files: every episode item stored in this file (primary first).
        let item_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT m.id FROM media_items m WHERE m.id IN (SELECT item_id FROM media_files \
             WHERE id = ?1 UNION SELECT item_id FROM media_file_links WHERE file_id = ?1) \
             ORDER BY m.episode_no, m.id",
        )
        .bind(fid)
        .fetch_all(&st.db)
        .await?;
        let streams = sqlx::query(
            "SELECT idx, kind, codec, width, height, hdr, bit_depth, channels, language, title, \
             \"default\" AS is_default, forced, external_path FROM streams WHERE file_id = ? \
             ORDER BY idx, id",
        )
        .bind(fid)
        .fetch_all(&st.db)
        .await?;
        let (mut video, mut audio, mut subtitles) = (None, Vec::new(), Vec::new());
        for s in &streams {
            let codec: String = s.get::<Option<String>, _>("codec").unwrap_or_default();
            let index: i64 = s.get("idx");
            match s.get::<String, _>("kind").as_str() {
                "video" if video.is_none() => {
                    video = Some(VideoStream {
                        codec,
                        width: s.get::<Option<i64>, _>("width").unwrap_or(0),
                        height: s.get::<Option<i64>, _>("height").unwrap_or(0),
                        hdr: Hdr::parse(&s.get::<String, _>("hdr")),
                        bit_depth: s.get::<Option<i64>, _>("bit_depth").unwrap_or(8),
                    })
                }
                "audio" => audio.push(AudioStream {
                    index,
                    codec,
                    channels: s.get::<Option<i64>, _>("channels").unwrap_or(0),
                    language: s.get("language"),
                    title: s.get("title"),
                    default: s.get::<i64, _>("is_default") != 0,
                }),
                "subtitle" => subtitles.push(SubtitleStream {
                    index,
                    codec,
                    language: s.get("language"),
                    title: s.get("title"),
                    forced: s.get::<i64, _>("forced") != 0,
                    external: s.get::<Option<String>, _>("external_path").is_some(),
                }),
                _ => {}
            }
        }
        files.push(MediaFile {
            id: fid,
            container: fr.get::<Option<String>, _>("container").unwrap_or_default(),
            duration_s: fr.get::<Option<f64>, _>("duration_s").unwrap_or(0.0),
            size: fr.get("size"),
            url: sg.url(
                &format!("/api/v1/direct/{fid}"),
                &format!("direct:{fid}"),
                st.media_ttl(),
                3600,
            ),
            video,
            audio,
            subtitles,
            item_ids: if item_ids.len() > 1 {
                item_ids
            } else {
                Vec::new()
            },
        });
    }
    Ok(files)
}

/// Children: show → seasons, season → episodes (ordered). Empty for leaf items.
#[utoipa::path(
    get,
    path = "/items/{id}/children",
    params(("id" = i64, Path, description = "Item id")),
    responses((status = 200, body = Vec<ItemSummary>), (status = 404, body = ErrorBody))
)]
pub async fn item_children(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<ItemSummary>>> {
    st.require_item(&u.user, id).await?;
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let rows = sqlx::query(&format!(
        "{sel}{frm} WHERE i.parent_id = ? \
         ORDER BY COALESCE(i.season_no, 0), COALESCE(i.episode_no, 0), i.id"
    ))
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(summaries(&st, &rows, &sg).await?))
}

// ───────────────────────── home / search ─────────────────────────

async fn recent_in(
    st: &AppState,
    uid: i64,
    vis: &Option<String>,
    library: Option<i64>,
) -> ApiResult<Vec<ItemSummary>> {
    let (sel, frm, sg) = (summary_select(uid), summary_from(uid), st.signer(uid));
    let mut sql = format!(
        "{sel}{frm} WHERE i.kind IN ('movie','show') AND {}",
        vis_clause("i.library_id")
    );
    if library.is_some() {
        sql.push_str(" AND i.library_id = ?");
    }
    sql.push_str(" ORDER BY i.added_at DESC, i.id DESC LIMIT ?");
    let mut q = sqlx::query(&sql).bind(vis).bind(vis);
    if let Some(l) = library {
        q = q.bind(l);
    }
    let rows = q.bind(ROW_LIMIT).fetch_all(&st.db).await?;
    summaries(st, &rows, &sg).await
}

/// Fills `series` on the episodes in `items` (a parent is a season or, for flat shows, the show).
async fn attach_series(
    st: &AppState,
    items: &mut [ItemSummary],
    sg: &reel_auth::UrlSigner,
) -> ApiResult<()> {
    let ids: Vec<i64> = items
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::Episode))
        .map(|i| i.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    type SeriesRow = (i64, i64, String, Option<String>, Option<String>);
    let rows: Vec<SeriesRow> = sqlx::query_as(
        "SELECT e.id, s.id, s.title, s.poster, s.backdrop FROM media_items e \
         JOIN media_items p ON p.id = e.parent_id \
         JOIN media_items s ON s.id = CASE WHEN p.kind = 'season' THEN p.parent_id ELSE p.id END \
         WHERE s.kind = 'show' AND e.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(&st.db)
    .await?;
    let mut by: std::collections::HashMap<i64, SeriesRef> = Default::default();
    for (episode, id, title, poster, backdrop) in rows {
        by.insert(
            episode,
            SeriesRef {
                id,
                title,
                poster_url: poster.map(|_| image_url(sg, "poster", id, 342)),
                backdrop_url: backdrop.map(|_| image_url(sg, "backdrop", id, 780)),
            },
        );
    }
    for i in items {
        i.series = by.remove(&i.id);
    }
    Ok(())
}

/// Home screen: continue watching, recently added, and one row per visible library.
#[utoipa::path(get, path = "/home", responses((status = 200, body = Home)))]
pub async fn home(State(st): State<AppState>, u: AuthUser) -> ApiResult<Json<Home>> {
    let visible = st.visible(&u.user).await?;
    let vis = vis_json(&visible);
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    // One entry per title: every film, and the most recently played in-progress episode of each
    // series. The partition key is the show's id for episodes and a negated item id otherwise, so
    // films and episodes without a parent never group with anything.
    let rows = sqlx::query(&format!(
        "WITH latest AS (SELECT m.id AS item_id, ROW_NUMBER() OVER (PARTITION BY \
            CASE WHEN m.kind = 'episode' AND p.id IS NOT NULL \
                THEN CASE WHEN p.kind = 'season' THEN COALESCE(p.parent_id, p.id) ELSE p.id END \
                ELSE -m.id END \
            ORDER BY COALESCE(ws.last_played_at, 0) DESC, m.id DESC) AS rn \
         FROM watch_state ws JOIN media_items m ON m.id = ws.item_id \
         LEFT JOIN media_items p ON p.id = m.parent_id \
         WHERE ws.user_id = ? AND ws.position_s > 0 AND ws.played = 0 \
         AND m.kind IN ('movie','episode') AND {}) \
         {sel}{frm} JOIN latest l ON l.item_id = i.id AND l.rn = 1 \
         ORDER BY COALESCE(w.last_played_at, 0) DESC, i.id DESC LIMIT ?",
        vis_clause("m.library_id")
    ))
    .bind(u.id())
    .bind(&vis)
    .bind(&vis)
    .bind(ROW_LIMIT)
    .fetch_all(&st.db)
    .await?;
    let mut continue_watching = summaries(&st, &rows, &sg).await?;
    attach_series(&st, &mut continue_watching, &sg).await?;
    let libs: Vec<(i64, String)> = sqlx::query_as(&format!(
        "SELECT id, name FROM libraries WHERE {} ORDER BY id",
        vis_clause("id")
    ))
    .bind(&vis)
    .bind(&vis)
    .fetch_all(&st.db)
    .await?;
    let mut out_rows = Vec::new();
    for (library_id, title) in libs {
        let items = recent_in(&st, u.id(), &vis, Some(library_id)).await?;
        out_rows.push(HomeRow {
            library_id,
            title,
            items,
        });
    }
    Ok(Json(Home {
        continue_watching,
        recently_added: recent_in(&st, u.id(), &vis, None).await?,
        rows: out_rows,
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchQuery {
    /// Search text (case-insensitive substring; prefix matches rank first).
    pub q: Option<String>,
    /// Max results (default 50, max 200).
    pub limit: Option<i64>,
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Library search over movie and show titles.
#[utoipa::path(
    get,
    path = "/search",
    params(SearchQuery),
    responses((status = 200, body = Vec<ItemSummary>))
)]
pub async fn search(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Vec<ItemSummary>>> {
    let text = q.q.unwrap_or_default().trim().to_lowercase();
    if text.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let esc = like_escape(&text);
    let vis = vis_json(&st.visible(&u.user).await?);
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let rows = sqlx::query(&format!(
        "{sel}{frm} WHERE i.kind IN ('movie','show') \
         AND lower(i.title) LIKE ? ESCAPE '\\' AND {} \
         ORDER BY CASE WHEN lower(i.title) LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END, \
         COALESCE(i.sort_title, lower(i.title)), i.id LIMIT ?",
        vis_clause("i.library_id")
    ))
    .bind(format!("%{esc}%"))
    .bind(&vis)
    .bind(&vis)
    .bind(format!("{esc}%"))
    .bind(q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
    .fetch_all(&st.db)
    .await?;
    Ok(Json(summaries(&st, &rows, &sg).await?))
}

// ───────────────────────── images ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct ImageQuery {
    /// Target width in px (snapped up to an allowed size); omitted = original.
    pub w: Option<u32>,
}

/// Resized, cached image (`poster`, `backdrop` or `person`).
#[utoipa::path(
    get,
    path = "/images/{kind}/{id}",
    params(
        ("kind" = String, Path, description = "poster | backdrop | person"),
        ("id" = i64, Path, description = "Item id (poster/backdrop) or person id"),
        ImageQuery,
        SignedQuery
    ),
    responses(
        (status = 200, description = "Image bytes", content_type = "image/jpeg"),
        (status = 304, description = "Not modified"),
        (status = 404, body = ErrorBody)
    )
)]
pub async fn image(
    State(st): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
    Query(q): Query<ImageQuery>,
    Query(sq): Query<SignedQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let user = st
        .media_user(&headers, &sq, &format!("img:{kind}:{id}"))
        .await?;
    if matches!(kind.as_str(), "poster" | "backdrop") {
        st.require_item(&user, id).await?;
    }
    let (source, ik): (Option<String>, ImageKind) = match kind.as_str() {
        "poster" => {
            let r: Option<(Option<String>, String)> =
                sqlx::query_as("SELECT poster, kind FROM media_items WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&st.db)
                    .await?;
            let (s, k) = r.ok_or(ApiError::NotFound)?;
            (
                s,
                if k == "episode" {
                    ImageKind::Still
                } else {
                    ImageKind::Poster
                },
            )
        }
        "backdrop" => (
            sqlx::query_scalar("SELECT backdrop FROM media_items WHERE id = ?")
                .bind(id)
                .fetch_optional(&st.db)
                .await?
                .ok_or(ApiError::NotFound)?,
            ImageKind::Backdrop,
        ),
        "person" => (
            sqlx::query_scalar("SELECT photo FROM people WHERE id = ?")
                .bind(id)
                .fetch_optional(&st.db)
                .await?
                .ok_or(ApiError::NotFound)?,
            ImageKind::Profile,
        ),
        _ => return Err(ApiError::NotFound),
    };
    let source = source.ok_or(ApiError::NotFound)?;
    // Local artwork (`file://`) may only be served from inside a library root.
    let roots = if source.starts_with("file://") {
        library_roots(&st.db).await?
    } else {
        Vec::new()
    };
    let path = st
        .images
        .get_in(ik, &source, q.w.unwrap_or(0), &roots)
        .await
        .map_err(|e| match e {
            reel_metadata::Error::Io(ref io)
                if io.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                tracing::warn!(%source, "local image rejected: {io}");
                ApiError::NotFound
            }
            reel_metadata::Error::Io(_) | reel_metadata::Error::Status(404) => ApiError::NotFound,
            e => {
                tracing::warn!(%source, "image fetch failed: {e}");
                ApiError::BadGateway("image unavailable".into())
            }
        })?;
    respond_file(path, &headers).await
}

/// All configured library root directories (from `libraries.paths_json`).
async fn library_roots(db: &sqlx::SqlitePool) -> ApiResult<Vec<std::path::PathBuf>> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT paths_json FROM libraries")
        .fetch_all(db)
        .await?;
    Ok(rows
        .iter()
        .flat_map(|j| serde_json::from_str::<Vec<String>>(j).unwrap_or_default())
        .map(std::path::PathBuf::from)
        .collect())
}

/// Serve a cached image file with ETag / long-lived cache headers.
async fn respond_file(path: std::path::PathBuf, headers: &HeaderMap) -> ApiResult<Response> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let etag = format!("\"{:x}-{:x}\"", meta.len(), mtime);
    let cache = HeaderValue::from_static("public, max-age=31536000");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        let mut res = StatusCode::NOT_MODIFIED.into_response();
        res.headers_mut().insert(header::CACHE_CONTROL, cache);
        if let Ok(v) = HeaderValue::from_str(&etag) {
            res.headers_mut().insert(header::ETAG, v);
        }
        return Ok(res);
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let ctype = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    let mut res = Body::from(bytes).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    h.insert(header::CACHE_CONTROL, cache);
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    Ok(res)
}

/// Resized, cached TMDB image by TMDB path (for discovery cards that aren't library items).
#[utoipa::path(
    get,
    path = "/images/tmdb/{kind}",
    params(
        ("kind" = String, Path, description = "poster | backdrop | profile"),
        TmdbImageQuery,
        SignedQuery
    ),
    responses(
        (status = 200, description = "Image bytes", content_type = "image/jpeg"),
        (status = 304, description = "Not modified"),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody)
    )
)]
pub async fn image_tmdb(
    State(st): State<AppState>,
    Path(kind): Path<String>,
    Query(q): Query<TmdbImageQuery>,
    Query(sq): Query<SignedQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    st.media_user(&headers, &sq, &format!("tmdb:{kind}:{}", q.path))
        .await?;
    let ik = match kind.as_str() {
        "poster" => ImageKind::Poster,
        "backdrop" => ImageKind::Backdrop,
        "profile" => ImageKind::Profile,
        _ => return Err(ApiError::NotFound),
    };
    let valid = q.path.len() < 100
        && q.path.starts_with('/')
        && q.path[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid || q.path.len() < 2 {
        return Err(ApiError::BadRequest("invalid TMDB image path".into()));
    }
    let path = st
        .images
        .get(ik, &q.path, q.w.unwrap_or(0))
        .await
        .map_err(|e| match e {
            reel_metadata::Error::Status(404) => ApiError::NotFound,
            e => {
                tracing::warn!(path = %q.path, "tmdb image fetch failed: {e}");
                ApiError::BadGateway("image unavailable".into())
            }
        })?;
    respond_file(path, &headers).await
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TmdbImageQuery {
    /// TMDB image path, e.g. `/abc123.jpg`.
    pub path: String,
    /// Target width in px; omitted = original.
    pub w: Option<u32>,
}

// ───────────────────────── direct play ─────────────────────────

pub(crate) fn container_mime(container: &str) -> &'static str {
    match container {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "mpegts" | "ts" => "video/mp2t",
        "wmv" | "asf" => "video/x-ms-wmv",
        _ => "application/octet-stream",
    }
}

/// Serve the original file with HTTP Range support.
#[utoipa::path(
    get,
    path = "/direct/{file_id}",
    params(("file_id" = i64, Path, description = "Media file id"), SignedQuery),
    responses(
        (status = 200, description = "Full file", content_type = "video/*"),
        (status = 206, description = "Partial content"),
        (status = 404, body = ErrorBody),
        (status = 416, description = "Range not satisfiable")
    )
)]
pub async fn direct(
    State(st): State<AppState>,
    Path(file_id): Path<i64>,
    req: Request,
) -> ApiResult<Response> {
    let sq = Query::<SignedQuery>::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or_default();
    let user = st
        .media_user(req.headers(), &sq, &format!("direct:{file_id}"))
        .await?;
    st.require_file(&user, file_id).await?;
    // media requests (any client) keep the playback session alive
    if let Some(sid) = req.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("s="))
            .map(str::to_string)
    }) {
        st.playback.touch_session(&sid);
    }
    let (path, container): (String, Option<String>) =
        sqlx::query_as("SELECT path, container FROM media_files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(ApiError::NotFound)?;
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(ApiError::NotFound);
    }
    let Ok(mut res) = ServeFile::new(path).oneshot(req).await;
    let ct = HeaderValue::from_static(container_mime(container.as_deref().unwrap_or("")));
    if res.status().is_success() {
        res.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    Ok(res.into_response())
}

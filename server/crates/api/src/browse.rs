//! Movie picker: filters, genres, browse lanes, "surprise me" and unified search.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Query, RawQuery, State},
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    auth::{AuthUser, vis_clause, vis_json},
    dto::{ItemKind, ItemSummary},
    error::{ApiError, ApiResult, ErrorBody},
    requests::{DiscoverCardDto, card_dto, tmdb_image},
    routes::{image_url, played_expr, summaries, summary_from, summary_select},
};

// ───────────────────────── filters ─────────────────────────

/// Runtime of a top-level item in seconds (own runtime, else its first file).
pub(crate) const RUNTIME_EXPR: &str = "COALESCE(i.runtime_s, (SELECT f.duration_s FROM media_files f \
     WHERE f.item_id = i.id ORDER BY f.part_no, f.id LIMIT 1))";

pub(crate) enum Bind {
    I(i64),
    F(f64),
    S(String),
}

pub(crate) fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    binds: &[Bind],
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    for b in binds {
        q = match b {
            Bind::I(i) => q.bind(*i),
            Bind::F(f) => q.bind(*f),
            Bind::S(s) => q.bind(s.clone()),
        };
    }
    q
}

/// Decoded `a=b&c=d` pairs (repeated keys preserved, in order).
pub(crate) fn parse_pairs(raw: Option<&str>) -> Vec<(String, String)> {
    form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Item filters shared by `/libraries/{id}/items`, `/pick` and the lanes' "see all" queries.
#[derive(Debug, Default, Clone)]
pub(crate) struct Filters {
    pub genres: Vec<String>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    pub min_rating: Option<f64>,
    pub max_runtime_min: Option<i64>,
    pub unwatched: bool,
}

fn num<T: std::str::FromStr>(k: &str, v: &str) -> ApiResult<T> {
    v.trim()
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("invalid value for '{k}'")))
}

impl Filters {
    pub fn from_pairs(pairs: &[(String, String)]) -> ApiResult<Self> {
        let mut f = Filters::default();
        for (k, v) in pairs {
            if v.trim().is_empty() {
                continue;
            }
            match k.as_str() {
                "genre" => f.genres.extend(
                    v.split(',')
                        .map(str::trim)
                        .filter(|g| !g.is_empty())
                        .map(str::to_string),
                ),
                "year_from" => f.year_from = Some(num(k, v)?),
                "year_to" => f.year_to = Some(num(k, v)?),
                "min_rating" => f.min_rating = Some(num(k, v)?),
                "max_runtime_min" => f.max_runtime_min = Some(num(k, v)?),
                "unwatched" => f.unwatched = matches!(v.trim(), "true" | "1" | "yes"),
                _ => {}
            }
        }
        Ok(f)
    }

    /// `" AND …"` fragments (aliases `i` = media_items, `w` = watch_state) and their binds.
    pub fn sql(&self, uid: i64) -> (String, Vec<Bind>) {
        let (mut s, mut b) = (String::new(), vec![]);
        let wanted: Vec<String> = {
            let mut g: Vec<String> = self.genres.iter().map(|g| g.to_lowercase()).collect();
            g.sort();
            g.dedup();
            g
        };
        if !wanted.is_empty() {
            s.push_str(
                " AND (SELECT count(DISTINCT lower(g.name)) FROM item_genres ig \
                 JOIN genres g ON g.id = ig.genre_id WHERE ig.item_id = i.id \
                 AND lower(g.name) IN (SELECT value FROM json_each(?))) = ?",
            );
            b.push(Bind::S(
                serde_json::to_string(&wanted).unwrap_or_else(|_| "[]".into()),
            ));
            b.push(Bind::I(wanted.len() as i64));
        }
        if let Some(y) = self.year_from {
            s.push_str(" AND i.year >= ?");
            b.push(Bind::I(y));
        }
        if let Some(y) = self.year_to {
            s.push_str(" AND i.year <= ?");
            b.push(Bind::I(y));
        }
        if let Some(r) = self.min_rating {
            s.push_str(" AND i.rating >= ?");
            b.push(Bind::F(r));
        }
        if let Some(m) = self.max_runtime_min {
            s.push_str(&format!(" AND {RUNTIME_EXPR} <= ?"));
            b.push(Bind::I(m * 60));
        }
        if self.unwatched {
            s.push_str(&format!(" AND ({}) = 0", played_expr(uid)));
        }
        (s, b)
    }
}

// ───────────────────────── text folding ─────────────────────────

pub(crate) use reel_core::text::fold;

/// 100 exact · 80 prefix · 60 word-prefix · 40 substring · 0 no match (both inputs folded).
pub(crate) fn text_score(text: &str, q: &str) -> u32 {
    if q.is_empty() || text.is_empty() {
        return 0;
    }
    if text == q {
        100
    } else if text.starts_with(q) {
        80
    } else if text.contains(&format!(" {q}")) {
        60
    } else if text.contains(q) {
        40
    } else {
        0
    }
}

/// 404 unless the library exists and the user may see it.
async fn check_library(st: &AppState, u: &AuthUser, id: i64) -> ApiResult<()> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE id = ?")
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    st.require_library(&u.user, id).await
}

// ───────────────────────── genres ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct GenresQuery {
    /// Restrict to one library (default: every library you can see).
    pub library_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GenreCount {
    pub name: String,
    /// Movies/shows (top-level items) with this genre.
    pub count: i64,
}

/// Genres with item counts, most common first.
#[utoipa::path(
    get,
    path = "/genres",
    params(GenresQuery),
    responses((status = 200, body = Vec<GenreCount>), (status = 404, body = ErrorBody))
)]
pub async fn list_genres(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<GenresQuery>,
) -> ApiResult<Json<Vec<GenreCount>>> {
    if let Some(l) = q.library_id {
        check_library(&st, &u, l).await?;
    }
    let vis = vis_json(&st.visible(&u.user).await?);
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT g.name, count(DISTINCT i.id) AS n FROM genres g \
         JOIN item_genres ig ON ig.genre_id = g.id \
         JOIN media_items i ON i.id = ig.item_id \
         WHERE i.kind IN ('movie','show') \
         AND (?1 IS NULL OR i.library_id IN (SELECT value FROM json_each(?2))) \
         AND (?3 IS NULL OR i.library_id = ?3) \
         GROUP BY g.id ORDER BY n DESC, g.name",
    )
    .bind(&vis)
    .bind(&vis)
    .bind(q.library_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(name, count)| GenreCount { name, count })
            .collect(),
    ))
}

// ───────────────────────── pick ─────────────────────────

/// One random movie/show matching the filters ("Surprise me").
#[utoipa::path(
    get,
    path = "/pick",
    params(
        ("library_id" = Option<i64>, Query, description = "Restrict to one library"),
        ("genre" = Option<String>, Query, description = "Genre(s), comma list or repeated"),
        ("max_runtime_min" = Option<i64>, Query, description = "Maximum runtime in minutes"),
        ("min_rating" = Option<f64>, Query, description = "Minimum rating 0-10"),
        ("unwatched" = Option<bool>, Query, description = "Only unwatched")
    ),
    responses((status = 200, body = ItemSummary), (status = 404, description = "Nothing matches", body = ErrorBody))
)]
pub async fn pick(
    State(st): State<AppState>,
    u: AuthUser,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<ItemSummary>> {
    let pairs = parse_pairs(raw.as_deref());
    let filters = Filters::from_pairs(&pairs)?;
    let library: Option<i64> = pairs
        .iter()
        .find(|(k, v)| k == "library_id" && !v.is_empty())
        .map(|(_, v)| num("library_id", v))
        .transpose()?;
    if let Some(l) = library {
        check_library(&st, &u, l).await?;
    }
    let vis = vis_json(&st.visible(&u.user).await?);
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let (fsql, fbinds) = filters.sql(u.id());
    let mut sql = format!(
        "{sel}{frm} WHERE i.kind IN ('movie','show') AND {}",
        vis_clause("i.library_id")
    );
    if library.is_some() {
        sql.push_str(" AND i.library_id = ?");
    }
    sql.push_str(&fsql);
    sql.push_str(" ORDER BY RANDOM() LIMIT 1");
    let mut q = sqlx::query(&sql).bind(&vis).bind(&vis);
    if let Some(l) = library {
        q = q.bind(l);
    }
    let rows = bind_all(q, &fbinds).fetch_all(&st.db).await?;
    summaries(&st, &rows, &sg)
        .await?
        .into_iter()
        .next()
        .map(Json)
        .ok_or(ApiError::NotFound)
}

// ───────────────────────── lanes ─────────────────────────

const LANE_ITEMS: usize = 20;
const MAX_LANES: usize = 14;
const MIN_LANE: usize = 3;
const MIN_GENRE_LANE: usize = 6;
const MIN_REDISCOVER: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    BecauseYouWatched,
    TopUnwatched,
    Short,
    New,
    Genre,
    Rediscover,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SeeAll {
    pub library_id: i64,
    /// Query string for `GET /libraries/{library_id}/items`.
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Lane {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub kind: LaneKind,
    /// At most 20 items.
    pub items: Vec<ItemSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub see_all: Option<SeeAll>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct LanesQuery {
    /// Library to build lanes for (default: the first library you can see).
    pub library_id: Option<i64>,
}

struct Cand {
    s: ItemSummary,
    tmdb: Option<i64>,
    last_played: Option<i64>,
}

impl Cand {
    fn rating(&self) -> f64 {
        self.s.rating.unwrap_or(0.0)
    }
}

/// splitmix64 → [0,1)
fn unit(seed: u64, id: i64) -> f64 {
    let mut z = seed
        .wrapping_add(id as u64)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn enc(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn by_rating_desc(a: &&Cand, b: &&Cand) -> std::cmp::Ordering {
    b.rating()
        .partial_cmp(&a.rating())
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.s.id.cmp(&b.s.id))
}

fn lane_items(c: &[&Cand]) -> Vec<ItemSummary> {
    c.iter().take(LANE_ITEMS).map(|c| c.s.clone()).collect()
}

/// Personalised browse lanes (per user: watch state; cached TMDB recommendations).
#[utoipa::path(
    get,
    path = "/browse/lanes",
    params(LanesQuery),
    responses((status = 200, body = Vec<Lane>), (status = 404, body = ErrorBody))
)]
pub async fn browse_lanes(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<LanesQuery>,
) -> ApiResult<Json<Vec<Lane>>> {
    let vis = vis_json(&st.visible(&u.user).await?);
    let lib: i64 = match q.library_id {
        Some(l) => {
            check_library(&st, &u, l).await?;
            l
        }
        None => {
            let first: Option<i64> = sqlx::query_scalar(&format!(
                "SELECT id FROM libraries WHERE {} ORDER BY id LIMIT 1",
                vis_clause("id")
            ))
            .bind(&vis)
            .bind(&vis)
            .fetch_optional(&st.db)
            .await?;
            match first {
                Some(l) => l,
                None => return Ok(Json(vec![])),
            }
        }
    };
    let (sel, frm, sg) = (
        summary_select(u.id()),
        summary_from(u.id()),
        st.signer(u.id()),
    );
    let rows: Vec<SqliteRow> = sqlx::query(&format!(
        "{sel}, i.tmdb_id AS tmdb, w.last_played_at AS lpa{frm} \
         WHERE i.library_id = ? AND i.kind IN ('movie','show')"
    ))
    .bind(lib)
    .fetch_all(&st.db)
    .await?;
    let items = summaries(&st, &rows, &sg).await?;
    let cands: Vec<Cand> = items
        .into_iter()
        .zip(&rows)
        .map(|(s, r)| Cand {
            s,
            tmdb: r.get("tmdb"),
            last_played: r.get("lpa"),
        })
        .collect();
    let see_all = |query: String| {
        Some(SeeAll {
            library_id: lib,
            query,
        })
    };
    let mut lanes: Vec<Lane> = vec![];

    // 1. Because you watched …
    if let Some(tmdb) = st.tmdb.clone() {
        let seeds = recent_seeds(&st, u.id(), &vis).await?;
        let by_tmdb: HashMap<(bool, i64), usize> = cands
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.tmdb.map(|t| ((c.s.kind == ItemKind::Show, t), i)))
            .collect();
        for (seed_id, title, tmdb_id, is_show) in seeds.into_iter().take(5) {
            if lanes.len() >= 2 {
                break;
            }
            let (recs, sim) = tokio::join!(
                tmdb.recommendations(is_show, tmdb_id),
                tmdb.similar(is_show, tmdb_id)
            );
            let mut seen = HashSet::new();
            let mut picked: Vec<&Cand> = vec![];
            for r in recs.into_iter().chain(sim).flat_map(|p| p.results) {
                if let Some(&i) = by_tmdb.get(&(is_show, r.id)) {
                    let c = &cands[i];
                    if c.s.id != seed_id && !c.s.played && seen.insert(c.s.id) {
                        picked.push(c);
                    }
                }
            }
            if picked.len() >= MIN_LANE {
                lanes.push(Lane {
                    key: format!("because_{seed_id}"),
                    title: format!("Because you watched {title}"),
                    subtitle: None,
                    kind: LaneKind::BecauseYouWatched,
                    items: lane_items(&picked),
                    see_all: None,
                });
            }
        }
    }

    // 2. Top rated you haven't seen
    let mut top: Vec<&Cand> = cands
        .iter()
        .filter(|c| !c.s.played && c.s.rating.is_some())
        .collect();
    top.sort_by(by_rating_desc);
    if top.len() >= MIN_LANE {
        lanes.push(Lane {
            key: "top_unwatched".into(),
            title: "Top rated you haven't seen".into(),
            subtitle: None,
            kind: LaneKind::TopUnwatched,
            items: lane_items(&top),
            see_all: see_all("sort=rating&unwatched=true".into()),
        });
    }

    // 3. Under 100 minutes (movies; unwatched first)
    let mut short: Vec<&Cand> = cands
        .iter()
        .filter(|c| {
            c.s.kind == ItemKind::Movie && c.s.runtime_s.is_some_and(|r| r > 0 && r <= 100 * 60)
        })
        .collect();
    short.sort_by(|a, b| {
        a.s.played
            .cmp(&b.s.played)
            .then_with(|| by_rating_desc(a, b))
    });
    if short.len() >= MIN_LANE {
        lanes.push(Lane {
            key: "short".into(),
            title: "Under 100 minutes".into(),
            subtitle: Some("Movies that fit a short evening".into()),
            kind: LaneKind::Short,
            items: lane_items(&short),
            see_all: see_all("max_runtime_min=100&sort=runtime".into()),
        });
    }

    // 4. New in your library
    let mut new: Vec<&Cand> = cands.iter().collect();
    new.sort_by(|a, b| b.s.added_at.cmp(&a.s.added_at).then(a.s.id.cmp(&b.s.id)));
    if new.len() >= MIN_LANE {
        lanes.push(Lane {
            key: "new".into(),
            title: "New in your library".into(),
            subtitle: None,
            kind: LaneKind::New,
            items: lane_items(&new),
            see_all: see_all("sort=added".into()),
        });
    }

    // 5. Rediscover: watched long ago, highly rated
    let cutoff = now_s() - 180 * 86_400;
    let mut redo: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.s.played && c.last_played.is_none_or(|t| t < cutoff) && c.rating() >= 7.0)
        .collect();
    redo.sort_by(by_rating_desc);
    if redo.len() >= MIN_REDISCOVER {
        lanes.push(Lane {
            key: "rediscover".into(),
            title: "Rediscover".into(),
            subtitle: Some("Favourites you haven't watched in a while".into()),
            kind: LaneKind::Rediscover,
            items: lane_items(&redo),
            see_all: None,
        });
    }

    // 6. One lane per genre (≥ 6 titles), most common first; light daily shuffle.
    let day = (now_s() / 86_400) as u64;
    let mut genres: HashMap<&str, Vec<&Cand>> = HashMap::new();
    for c in &cands {
        for g in c.s.genres.iter().flatten() {
            genres.entry(g.as_str()).or_default().push(c);
        }
    }
    let mut genres: Vec<(&str, Vec<&Cand>)> = genres
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_GENRE_LANE)
        .collect();
    genres.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (name, mut v) in genres {
        if lanes.len() >= MAX_LANES {
            break;
        }
        let score = |c: &Cand| {
            c.s.rating.unwrap_or(5.0) + unit(day ^ (name.len() as u64) << 32, c.s.id) * 2.0
        };
        v.sort_by(|a, b| {
            score(b)
                .partial_cmp(&score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        lanes.push(Lane {
            key: format!("genre_{}", fold(name).replace(' ', "_")),
            title: name.to_string(),
            subtitle: Some(format!("{} titles", v.len())),
            kind: LaneKind::Genre,
            items: lane_items(&v),
            see_all: see_all(format!("genre={}&sort=rating", enc(name))),
        });
    }
    lanes.truncate(MAX_LANES);
    Ok(Json(lanes))
}

/// The user's most recently played movies/shows (episodes count for their show):
/// `(item id, title, tmdb id, is_show)`.
async fn recent_seeds(
    st: &AppState,
    uid: i64,
    vis: &Option<String>,
) -> ApiResult<Vec<(i64, String, i64, bool)>> {
    let rows: Vec<(i64, String, i64, String)> = sqlx::query_as(&format!(
        "SELECT t.id, t.title, t.tmdb_id, t.kind FROM ( \
           SELECT CASE WHEN i.kind = 'episode' THEN COALESCE( \
                    (SELECT p.parent_id FROM media_items p WHERE p.id = i.parent_id AND p.kind = 'season'), \
                    i.parent_id) ELSE i.id END AS top, max(w.last_played_at) AS lp \
           FROM watch_state w JOIN media_items i ON i.id = w.item_id \
           WHERE w.user_id = ? AND w.last_played_at IS NOT NULL AND i.kind IN ('movie','episode') \
             AND {} GROUP BY top) s \
         JOIN media_items t ON t.id = s.top \
         WHERE t.kind IN ('movie','show') AND t.tmdb_id IS NOT NULL ORDER BY s.lp DESC LIMIT 8",
        vis_clause("i.library_id")
    ))
    .bind(uid)
    .bind(vis)
    .bind(vis)
    .fetch_all(&st.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, tmdb, kind)| (id, title, tmdb, kind == "show"))
        .collect())
}

// ───────────────────────── search ─────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchAllQuery {
    /// Search text.
    pub q: Option<String>,
    /// Max library results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PersonHit {
    pub tmdb_id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_for_department: Option<String>,
    /// Titles this person is known for (library titles for local matches).
    pub known_for: Vec<String>,
    /// Movies/shows in your library they are credited on.
    pub in_library_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SearchAll {
    /// Movies and shows: title matches first, then cast/crew-name matches.
    pub library: Vec<ItemSummary>,
    /// Episode title matches (≤ 10).
    pub episodes: Vec<ItemSummary>,
    /// Library cast/crew merged with TMDB people.
    pub people: Vec<PersonHit>,
    /// TMDB movies/shows that are not in your library (status/progress as in discovery).
    pub discover: Vec<DiscoverCardDto>,
    /// Xtream VOD movies and series matching the text.
    pub vod: Vec<crate::vod::VodCard>,
}

/// `(id, name, tmdb_id, photo)` of a local person.
type PersonRow = (i64, String, Option<i64>, Option<String>);

/// One search box for everything: your library (titles and cast), episodes, people and TMDB.
#[utoipa::path(
    get,
    path = "/search/all",
    params(SearchAllQuery),
    responses((status = 200, body = SearchAll))
)]
pub async fn search_all(
    State(st): State<AppState>,
    u: AuthUser,
    Query(q): Query<SearchAllQuery>,
) -> ApiResult<Json<SearchAll>> {
    let raw = q.q.unwrap_or_default();
    let raw = raw.trim();
    let needle = fold(raw);
    if needle.is_empty() {
        return Ok(Json(SearchAll {
            library: vec![],
            episodes: vec![],
            people: vec![],
            discover: vec![],
            vod: vec![],
        }));
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 100) as usize;
    let vis = vis_json(&st.visible(&u.user).await?);
    let sg = st.signer(u.id());

    // ── library: titles + cast/crew names ──
    let titles: Vec<(i64, String, f64)> = sqlx::query_as(&format!(
        "SELECT i.id, i.title, CAST(COALESCE(i.rating, 0) AS REAL) FROM media_items i \
         WHERE i.kind IN ('movie','show') AND {}",
        vis_clause("i.library_id")
    ))
    .bind(&vis)
    .bind(&vis)
    .fetch_all(&st.db)
    .await?;
    let mut score: HashMap<i64, (u32, f64, String)> = HashMap::new();
    for (id, title, rating) in &titles {
        let ft = fold(title);
        let s = text_score(&ft, &needle);
        if s > 0 {
            score.insert(*id, (s, *rating, ft));
        }
    }
    let ratings: HashMap<i64, (f64, &str)> = titles
        .iter()
        .map(|(id, t, r)| (*id, (*r, t.as_str())))
        .collect();
    // people whose name matches
    let people: Vec<PersonRow> = sqlx::query_as("SELECT id, name, tmdb_id, photo FROM people")
        .fetch_all(&st.db)
        .await?;
    let mut matched: HashMap<i64, u32> = HashMap::new();
    for (id, name, ..) in &people {
        let s = text_score(&fold(name), &needle);
        if s > 0 {
            matched.insert(*id, s);
        }
    }
    if !matched.is_empty() {
        let ids = serde_json::to_string(&matched.keys().collect::<Vec<_>>()).unwrap_or_default();
        let credits: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT DISTINCT c.person_id, c.item_id FROM credits c \
             JOIN media_items i ON i.id = c.item_id \
             WHERE c.person_id IN (SELECT value FROM json_each(?)) \
             AND i.kind IN ('movie','show') AND {}",
            vis_clause("i.library_id")
        ))
        .bind(ids)
        .bind(&vis)
        .bind(&vis)
        .fetch_all(&st.db)
        .await?;
        for (_, item) in &credits {
            if let Some((r, t)) = ratings.get(item) {
                score.entry(*item).or_insert((20, *r, fold(t)));
            }
        }
    }
    let mut order: Vec<(i64, u32, f64, String)> = score
        .into_iter()
        .map(|(id, (s, r, t))| (id, s, r, t))
        .collect();
    order.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.3.cmp(&b.3))
            .then(a.0.cmp(&b.0))
    });
    order.truncate(limit);
    let (sel, frm) = (summary_select(u.id()), summary_from(u.id()));
    let library = if order.is_empty() {
        vec![]
    } else {
        let ids = serde_json::to_string(&order.iter().map(|o| o.0).collect::<Vec<_>>())
            .unwrap_or_default();
        let rows = sqlx::query(&format!(
            "{sel}{frm} WHERE i.id IN (SELECT value FROM json_each(?))"
        ))
        .bind(ids)
        .fetch_all(&st.db)
        .await?;
        let mut by: HashMap<i64, ItemSummary> = summaries(&st, &rows, &sg)
            .await?
            .into_iter()
            .map(|s| (s.id, s))
            .collect();
        order.iter().filter_map(|o| by.remove(&o.0)).collect()
    };

    // ── episodes ──
    let like = format!(
        "%{}%",
        raw.to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows = sqlx::query(&format!(
        "{sel}{frm} WHERE i.kind = 'episode' AND lower(i.title) LIKE ? ESCAPE '\\' AND {} \
         ORDER BY CASE WHEN lower(i.title) LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END, \
         COALESCE(i.sort_title, lower(i.title)), i.id LIMIT 10",
        vis_clause("i.library_id")
    ))
    .bind(&like)
    .bind(&vis)
    .bind(&vis)
    .bind(format!(
        "{}%",
        like.trim_start_matches('%').trim_end_matches('%')
    ))
    .fetch_all(&st.db)
    .await?;
    let episodes = summaries(&st, &rows, &sg).await?;

    // ── TMDB (people + titles); failures only drop these sections ──
    let tmdb_part = async {
        let d = st.requests.discovery();
        let t = std::time::Duration::from_secs(5);
        let (p, c) = tokio::join!(
            tokio::time::timeout(t, d.search_people(raw)),
            tokio::time::timeout(t, d.search_cached(raw, None))
        );
        (
            p.ok().and_then(Result::ok).unwrap_or_default(),
            c.ok().and_then(Result::ok).unwrap_or_default(),
        )
    };
    let (tmdb_people, cards) = tmdb_part.await;

    // ── people: local ∪ TMDB ──
    let tmdb_ids: Vec<i64> = tmdb_people.iter().map(|p| p.id).collect();
    let mut person_rows: HashMap<i64, &PersonRow> = HashMap::new();
    for p in &people {
        if matched.contains_key(&p.0) || p.2.is_some_and(|t| tmdb_ids.contains(&t)) {
            person_rows.insert(p.0, p);
        }
    }
    let mut credit_map: HashMap<i64, Vec<i64>> = HashMap::new();
    if !person_rows.is_empty() {
        let ids =
            serde_json::to_string(&person_rows.keys().collect::<Vec<_>>()).unwrap_or_default();
        let rows: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT DISTINCT c.person_id, c.item_id FROM credits c \
             JOIN media_items i ON i.id = c.item_id \
             WHERE c.person_id IN (SELECT value FROM json_each(?)) \
             AND i.kind IN ('movie','show') AND {}",
            vis_clause("i.library_id")
        ))
        .bind(ids)
        .bind(&vis)
        .bind(&vis)
        .fetch_all(&st.db)
        .await?;
        for (p, i) in rows {
            credit_map.entry(p).or_default().push(i);
        }
    }
    let known_titles = |person: i64| -> Vec<String> {
        let mut v: Vec<(f64, &str)> = credit_map
            .get(&person)
            .into_iter()
            .flatten()
            .filter_map(|i| ratings.get(i).copied())
            .collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().take(3).map(|(_, t)| t.to_string()).collect()
    };
    let mut hits: Vec<PersonHit> = vec![];
    let mut used: HashSet<i64> = HashSet::new();
    for tp in &tmdb_people {
        let local = person_rows.values().find(|p| p.2 == Some(tp.id));
        let count = local
            .map(|p| credit_map.get(&p.0).map_or(0, Vec::len) as i64)
            .unwrap_or(0);
        used.insert(tp.id);
        hits.push(PersonHit {
            tmdb_id: tp.id,
            name: tp.name.clone(),
            photo_url: tmdb_image(&sg, "profile", &tp.profile_path, 185),
            known_for_department: tp.known_for_department.clone(),
            known_for: tp
                .known_for
                .iter()
                .map(|k| k.display_title().to_string())
                .filter(|t| !t.is_empty())
                .take(3)
                .collect(),
            in_library_count: count,
        });
    }
    for p in person_rows.values() {
        let Some(tid) = p.2 else { continue };
        let count = credit_map.get(&p.0).map_or(0, Vec::len) as i64;
        if used.contains(&tid) || count == 0 {
            continue;
        }
        hits.push(PersonHit {
            tmdb_id: tid,
            name: p.1.clone(),
            photo_url: p.3.as_ref().map(|_| image_url(&sg, "person", p.0, 185)),
            known_for_department: None,
            known_for: known_titles(p.0),
            in_library_count: count,
        });
    }
    // people you have in the library first (keeping TMDB relevance order inside each group)
    let mut idx: Vec<usize> = (0..hits.len()).collect();
    idx.sort_by_key(|&i| (hits[i].in_library_count == 0, i));
    let people_out: Vec<PersonHit> = idx.into_iter().take(8).map(|i| hits[i].clone()).collect();

    // ── discover: not already in (a visible part of) the library ──
    let mut discover = vec![];
    // discover cards lead to requests: only for accounts that may request
    let mut dtos: Vec<_> = cards
        .iter()
        .take(if u.user.can_request() { 40 } else { 0 })
        .map(|c| card_dto(c, &sg))
        .collect();
    crate::requests::scope_cards(&st, &u.user, &mut dtos).await?;
    for dto in dtos {
        // in a library this user can open: already listed above
        if dto.library_item_id.is_some() {
            continue;
        }
        discover.push(dto);
        if discover.len() >= 12 {
            break;
        }
    }
    Ok(Json(SearchAll {
        library,
        episodes,
        people: people_out,
        discover,
        vod: if u.user.can_vod() {
            crate::vod::search_cards(&st, &u, &needle, 12).await?
        } else {
            vec![]
        },
    }))
}

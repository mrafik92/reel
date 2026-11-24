//! Xtream VOD: movie + series catalog ingest, lazy `get_vod_info` / `get_series_info`, and the
//! URLs/targets playback needs. Stable ids (rows are diffed by the provider's ids), watch state
//! lives elsewhere (`vod_watch`) so a catalog refresh never loses it.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::{
    Error, Result,
    ingest::Ingest,
    store::{self, Source, SourceKind},
    xtream::{XtreamClient, flex_i64, flex_str},
};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Provider names look like `AR| Some Movie (2019)` or `[EN] Show - 4K`: returns the display
/// title and a year found in it.
pub fn clean_title(name: &str) -> (String, Option<i32>) {
    let mut t = name.trim().to_string();
    // leading country/provider tag: "AR|", "UK:", "[EN]", "| AR |"
    loop {
        let trimmed = t.trim_start_matches(['|', ' ', '-']);
        let tag: String = trimmed
            .trim_start_matches('[')
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        let after = &trimmed[trimmed.len() - trimmed.trim_start_matches('[').len()..];
        let rest = &after[tag.len().min(after.len())..];
        let bracketed = trimmed.starts_with('[');
        let is_tag = (2..=4).contains(&tag.len())
            && tag.chars().all(|c| c.is_ascii_uppercase())
            && (bracketed && rest.starts_with(']')
                || rest.trim_start().starts_with(['|', ':'])
                || rest.starts_with(" - "));
        if !is_tag {
            t = trimmed.to_string();
            break;
        }
        let cut = rest
            .trim_start_matches([']', ' '])
            .trim_start_matches(['|', ':', '-']);
        t = cut.trim().to_string();
    }
    // trailing "(2019)" / "[2019]"
    let mut year = None;
    for (open, close) in [('(', ')'), ('[', ']')] {
        if t.ends_with(close) {
            if let Some(i) = t.rfind(open) {
                let inner = &t[i + 1..t.len() - 1];
                if inner.len() == 4 && inner.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(y) = inner.parse::<i32>() {
                        if (1888..=2100).contains(&y) {
                            year = Some(y);
                            t = t[..i].trim().to_string();
                        }
                    }
                }
            }
        }
    }
    if t.is_empty() {
        t = name.trim().to_string();
    }
    (t, year)
}

fn year_of(v: &Value) -> Option<i32> {
    for k in [
        "year",
        "releasedate",
        "releaseDate",
        "release_date",
        "releaseDate",
    ] {
        if let Some(s) = v.get(k).and_then(flex_str) {
            if let Some(y) = s.get(..4).and_then(|y| y.parse::<i32>().ok()) {
                if (1888..=2100).contains(&y) {
                    return Some(y);
                }
            }
        }
    }
    None
}

fn rating_of(v: &Value) -> Option<f64> {
    let r = v
        .get("rating")
        .and_then(|r| match r {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .filter(|r| *r > 0.0)?;
    // some panels send a 0-5 scale in `rating_5based` only; ratings above 10 are percentages
    Some(if r > 10.0 { (r / 10.0).min(10.0) } else { r })
}

fn first_image(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::Array(a) => a.iter().find_map(flex_str),
        other => flex_str(other),
    }
    .filter(|u| u.starts_with("http"))
}

fn tmdb_of(v: &Value) -> Option<i64> {
    ["tmdb_id", "tmdb"]
        .iter()
        .find_map(|k| v.get(*k).and_then(flex_i64))
        .filter(|i| *i > 0)
}

/// `"a / b, c"` → `"a,b,c"`.
fn genres_of(s: &str) -> Option<String> {
    let g: Vec<&str> = s
        .split(['/', ',', '|'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect();
    (!g.is_empty()).then(|| g.join(","))
}

/// `https://www.youtube.com/watch?v=ID`, `youtu.be/ID` or a bare id.
pub fn youtube_id(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let id = if let Some(i) = s.find("v=") {
        s[i + 2..].split(['&', '#']).next().unwrap_or("")
    } else if let Some(i) = s.find("youtu.be/") {
        s[i + 9..].split(['?', '&', '#']).next().unwrap_or("")
    } else {
        s
    };
    (id.len() >= 6
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then(|| id.to_string())
}

fn duration_secs(v: &Value) -> Option<i64> {
    if let Some(n) = v.get("duration_secs").and_then(flex_i64).filter(|n| *n > 0) {
        return Some(n);
    }
    let d = v.get("duration").and_then(flex_str)?;
    let p: Vec<i64> = d.split(':').filter_map(|x| x.trim().parse().ok()).collect();
    match p.as_slice() {
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        [m, s] => Some(m * 60 + s),
        _ => None,
    }
    .filter(|n| *n > 0)
}

struct Cat {
    ext: String,
    name: String,
}

fn cats_of(v: &Value) -> Option<Vec<Cat>> {
    Some(
        v.as_array()?
            .iter()
            .filter_map(|c| {
                Some(Cat {
                    ext: flex_str(c.get("category_id")?)?,
                    name: flex_str(c.get("category_name")?)?,
                })
            })
            .collect(),
    )
}

impl Ingest {
    /// Catalog ingest of a source's VOD movies + series. Needs an Xtream source; failures are
    /// logged by the caller and never fail a source refresh.
    pub async fn refresh_vod(&self, src: &Source) -> Result<(usize, usize)> {
        if src.kind != SourceKind::Xtream {
            return Ok((0, 0));
        }
        let x = self.xtream_for(src)?;
        let movies = self.ingest_kind(src, &x, "movie").await?;
        let series = self.ingest_kind(src, &x, "series").await?;
        sqlx::query(
            "UPDATE iptv_sources SET last_vod_at = ?2, \
             vod_movie_count = (SELECT count(*) FROM vod_movies WHERE source_id = ?1 AND missing = 0), \
             vod_series_count = (SELECT count(*) FROM vod_series WHERE source_id = ?1 AND missing = 0) \
             WHERE id = ?1",
        )
        .bind(src.id)
        .bind(now())
        .execute(&self.pool)
        .await
        .ok();
        info!(source = src.id, movies, series, "VOD catalog refreshed");
        Ok((movies, series))
    }

    async fn ingest_kind(&self, src: &Source, x: &XtreamClient, kind: &str) -> Result<usize> {
        let (cat_action, list_action) = if kind == "movie" {
            ("get_vod_categories", "get_vod_streams")
        } else {
            ("get_series_categories", "get_series")
        };
        let cats = x.action(cat_action, &[]).await?;
        let list = x.action(list_action, &[]).await?;
        // an error object / `false` must not wipe the catalog
        let (Some(cats), Some(items)) = (cats_of(&cats), list.as_array()) else {
            warn!(
                source = src.id,
                kind, "provider returned no VOD list; keeping the old catalog"
            );
            return Ok(0);
        };
        crate::limits::check_count(
            "the provider's VOD category list",
            cats.len(),
            crate::limits::MAX_CATEGORIES,
        )?;
        crate::limits::check_count(
            "the provider's VOD list",
            items.len(),
            crate::limits::MAX_VOD_ITEMS,
        )?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut cat_ids: HashMap<String, i64> = HashMap::new();
        for c in &cats {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO vod_categories (source_id, kind, ext_id, name) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(source_id, kind, ext_id) DO UPDATE SET name = excluded.name RETURNING id",
            )
            .bind(src.id)
            .bind(kind)
            .bind(&c.ext)
            .bind(&c.name)
            .fetch_one(&mut *tx)
            .await?;
            cat_ids.insert(c.ext.clone(), id);
        }
        let (table, idcol) = if kind == "movie" {
            ("vod_movies", "stream_id")
        } else {
            ("vod_series", "series_id")
        };
        sqlx::query(&format!(
            "UPDATE {table} SET missing = 1 WHERE source_id = ?"
        ))
        .bind(src.id)
        .execute(&mut *tx)
        .await?;
        let mut n = 0usize;
        for it in items {
            let ext_id = if kind == "movie" {
                it.get("stream_id").and_then(flex_i64)
            } else {
                it.get("series_id").and_then(flex_i64)
            };
            let (Some(ext_id), Some(name)) = (ext_id, it.get("name").and_then(flex_str)) else {
                continue;
            };
            let (title, name_year) = clean_title(&name);
            let year = year_of(it).or(name_year);
            let norm = reel_core::text::fold(&title);
            let cat = it
                .get("category_id")
                .and_then(flex_str)
                .and_then(|c| cat_ids.get(&c).copied());
            let poster = if kind == "movie" {
                it.get("stream_icon").and_then(flex_str)
            } else {
                it.get("cover").and_then(flex_str)
            }
            .filter(|u| u.starts_with("http"));
            let added = it
                .get("added")
                .or_else(|| it.get("last_modified"))
                .and_then(flex_i64)
                .filter(|a| *a > 0);
            let rating = rating_of(it);
            let tmdb = tmdb_of(it);
            let genres = it
                .get("genre")
                .and_then(flex_str)
                .and_then(|g| genres_of(&g));
            if kind == "movie" {
                let container = it.get("container_extension").and_then(flex_str);
                sqlx::query(&format!(
                    "INSERT INTO vod_movies (source_id, stream_id, category_id, title, title_norm, year, \
                     rating, poster, container, added_at, tmdb_id, genres) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) \
                     ON CONFLICT(source_id, {idcol}) DO UPDATE SET category_id = excluded.category_id, \
                     title = excluded.title, title_norm = excluded.title_norm, \
                     year = COALESCE(excluded.year, year), rating = COALESCE(excluded.rating, rating), \
                     poster = COALESCE(excluded.poster, poster), \
                     container = COALESCE(excluded.container, container), \
                     added_at = COALESCE(excluded.added_at, added_at), \
                     tmdb_id = COALESCE(excluded.tmdb_id, tmdb_id), \
                     genres = COALESCE(excluded.genres, genres), missing = 0"
                ))
                .bind(src.id)
                .bind(ext_id)
                .bind(cat)
                .bind(&title)
                .bind(&norm)
                .bind(year)
                .bind(rating)
                .bind(&poster)
                .bind(&container)
                .bind(added)
                .bind(tmdb)
                .bind(genres)
                .execute(&mut *tx)
                .await?;
            } else {
                let backdrop = first_image(it, "backdrop_path");
                sqlx::query(&format!(
                    "INSERT INTO vod_series (source_id, series_id, category_id, title, title_norm, year, \
                     rating, poster, backdrop, added_at, tmdb_id, genres, overview) \
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?) \
                     ON CONFLICT(source_id, {idcol}) DO UPDATE SET category_id = excluded.category_id, \
                     title = excluded.title, title_norm = excluded.title_norm, \
                     year = COALESCE(excluded.year, year), rating = COALESCE(excluded.rating, rating), \
                     poster = COALESCE(excluded.poster, poster), \
                     backdrop = COALESCE(excluded.backdrop, backdrop), \
                     added_at = COALESCE(excluded.added_at, added_at), \
                     tmdb_id = COALESCE(excluded.tmdb_id, tmdb_id), \
                     genres = COALESCE(excluded.genres, genres), \
                     overview = COALESCE(overview, excluded.overview), missing = 0"
                ))
                .bind(src.id)
                .bind(ext_id)
                .bind(cat)
                .bind(&title)
                .bind(&norm)
                .bind(year)
                .bind(rating)
                .bind(&poster)
                .bind(&backdrop)
                .bind(added)
                .bind(tmdb)
                .bind(&genres)
                .bind(it.get("plot").and_then(flex_str))
                .execute(&mut *tx)
                .await?;
            }
            n += 1;
        }
        tx.commit().await?;
        Ok(n)
    }

    fn xtream_for(&self, src: &Source) -> Result<XtreamClient> {
        let (Some(u), Some(p)) = (&src.username, &src.password) else {
            return Err(Error::Invalid(
                "Xtream sources need a username and password".into(),
            ));
        };
        crate::net::check_url(&src.url, self.allow_private)?;
        Ok(XtreamClient::new(self.http.clone(), &src.url, u, p)
            .with_user_agent(src.user_agent.clone())
            .with_gate(self.gates.get(src.id)))
    }

    /// Lazily load `get_vod_info` for a movie (cached in the row; re-fetched after a week).
    pub async fn ensure_movie_info(&self, movie_id: i64) -> Result<()> {
        let row: Option<(i64, i64, Option<i64>)> =
            sqlx::query_as("SELECT source_id, stream_id, info_at FROM vod_movies WHERE id = ?")
                .bind(movie_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((source_id, stream_id, info_at)) = row else {
            return Err(Error::NotFound(format!("movie {movie_id}")));
        };
        if info_at.is_some_and(|t| now() - t < 7 * 86_400) {
            return Ok(());
        }
        let src = store::get_source(&self.pool, source_id).await?;
        let x = self
            .xtream_for(&src)?
            .with_max_wait(std::time::Duration::from_secs(1));
        let v = x
            .action("get_vod_info", &[("vod_id", stream_id.to_string())])
            .await?;
        let info = v.get("info").cloned().unwrap_or(Value::Null);
        let data = v.get("movie_data").cloned().unwrap_or(Value::Null);
        let tmdb = tmdb_of(&info);
        let cast = info
            .get("cast")
            .or_else(|| info.get("actors"))
            .and_then(flex_str)
            .map(|c| names_json(&c));
        let backdrop = first_image(&info, "backdrop_path");
        let poster = ["cover_big", "movie_image"]
            .iter()
            .find_map(|k| info.get(*k).and_then(flex_str))
            .filter(|u| u.starts_with("http"));
        sqlx::query(
            "UPDATE vod_movies SET overview = COALESCE(?, overview), backdrop = COALESCE(?, backdrop), \
             poster = COALESCE(poster, ?), runtime_s = COALESCE(?, runtime_s), \
             director = COALESCE(?, director), cast_json = COALESCE(?, cast_json), \
             trailer = COALESCE(?, trailer), genres = COALESCE(?, genres), \
             tmdb_id = COALESCE(?, tmdb_id), year = COALESCE(year, ?), \
             rating = COALESCE(rating, ?), container = COALESCE(?, container), info_at = ? \
             WHERE id = ?",
        )
        .bind(
            ["plot", "description"]
                .iter()
                .find_map(|k| info.get(*k).and_then(flex_str)),
        )
        .bind(backdrop)
        .bind(poster)
        .bind(duration_secs(&info))
        .bind(info.get("director").and_then(flex_str))
        .bind(cast)
        .bind(
            info.get("youtube_trailer")
                .and_then(flex_str)
                .and_then(|t| youtube_id(&t)),
        )
        .bind(info.get("genre").and_then(flex_str).and_then(|g| genres_of(&g)))
        .bind(tmdb)
        .bind(year_of(&info))
        .bind(rating_of(&info))
        .bind(data.get("container_extension").and_then(flex_str))
        .bind(now())
        .bind(movie_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lazily load `get_series_info` (series details + seasons + episodes).
    pub async fn ensure_series_info(&self, series_pk: i64) -> Result<()> {
        let row: Option<(i64, i64, Option<i64>)> =
            sqlx::query_as("SELECT source_id, series_id, info_at FROM vod_series WHERE id = ?")
                .bind(series_pk)
                .fetch_optional(&self.pool)
                .await?;
        let Some((source_id, series_id, info_at)) = row else {
            return Err(Error::NotFound(format!("series {series_pk}")));
        };
        let have_eps: i64 =
            sqlx::query_scalar("SELECT count(*) FROM vod_episodes WHERE series_pk = ?")
                .bind(series_pk)
                .fetch_one(&self.pool)
                .await?;
        if have_eps > 0 && info_at.is_some_and(|t| now() - t < 24 * 3600) {
            return Ok(());
        }
        let src = store::get_source(&self.pool, source_id).await?;
        let x = self
            .xtream_for(&src)?
            .with_max_wait(std::time::Duration::from_secs(1));
        let v = x
            .action("get_series_info", &[("series_id", series_id.to_string())])
            .await?;
        let info = v.get("info").cloned().unwrap_or(Value::Null);
        let cast = info.get("cast").and_then(flex_str).map(|c| names_json(&c));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE vod_series SET overview = COALESCE(?, overview), backdrop = COALESCE(?, backdrop), \
             poster = COALESCE(poster, ?), director = COALESCE(?, director), \
             cast_json = COALESCE(?, cast_json), trailer = COALESCE(?, trailer), \
             genres = COALESCE(?, genres), tmdb_id = COALESCE(?, tmdb_id), year = COALESCE(year, ?), \
             rating = COALESCE(rating, ?), info_at = ? WHERE id = ?",
        )
        .bind(info.get("plot").and_then(flex_str))
        .bind(first_image(&info, "backdrop_path"))
        .bind(info.get("cover").and_then(flex_str).filter(|u| u.starts_with("http")))
        .bind(info.get("director").and_then(flex_str))
        .bind(cast)
        .bind(
            info.get("youtube_trailer")
                .and_then(flex_str)
                .and_then(|t| youtube_id(&t)),
        )
        .bind(info.get("genre").and_then(flex_str).and_then(|g| genres_of(&g)))
        .bind(tmdb_of(&info))
        .bind(year_of(&info))
        .bind(rating_of(&info))
        .bind(now())
        .bind(series_pk)
        .execute(&mut *tx)
        .await?;
        if let Some(seasons) = v.get("seasons").and_then(Value::as_array) {
            for s in seasons {
                let Some(no) = s.get("season_number").and_then(flex_i64) else {
                    continue;
                };
                sqlx::query(
                    "INSERT INTO vod_seasons (series_pk, season_no, name, poster) VALUES (?,?,?,?) \
                     ON CONFLICT(series_pk, season_no) DO UPDATE SET name = excluded.name, \
                     poster = COALESCE(excluded.poster, poster)",
                )
                .bind(series_pk)
                .bind(no)
                .bind(s.get("name").and_then(flex_str))
                .bind(
                    ["cover_big", "cover"]
                        .iter()
                        .find_map(|k| s.get(*k).and_then(flex_str))
                        .filter(|u| u.starts_with("http")),
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        // `episodes` is `{ "1": [ep, …], "2": […] }` (or a plain array for odd panels)
        let mut seen: Vec<i64> = vec![];
        let groups: Vec<&Value> = match v.get("episodes") {
            Some(Value::Object(m)) => m.values().collect(),
            Some(a @ Value::Array(_)) => vec![a],
            _ => vec![],
        };
        for g in groups {
            let list: Vec<&Value> = match g {
                Value::Array(a) => a
                    .iter()
                    .flat_map(|e| match e {
                        Value::Array(inner) => inner.iter().collect::<Vec<_>>(),
                        other => vec![other],
                    })
                    .collect(),
                _ => vec![],
            };
            for e in list {
                let Some(ext_id) = e.get("id").and_then(flex_i64) else {
                    continue;
                };
                let einfo = e.get("info").cloned().unwrap_or(Value::Null);
                let season = e
                    .get("season")
                    .and_then(flex_i64)
                    .or_else(|| einfo.get("season").and_then(flex_i64))
                    .unwrap_or(1);
                let ep_no = e.get("episode_num").and_then(flex_i64).unwrap_or(0);
                let title = e
                    .get("title")
                    .and_then(flex_str)
                    .unwrap_or_else(|| format!("Episode {ep_no}"));
                sqlx::query(
                    "INSERT INTO vod_episodes (series_pk, ext_id, season_no, episode_no, title, \
                     overview, still, runtime_s, container) VALUES (?,?,?,?,?,?,?,?,?) \
                     ON CONFLICT(series_pk, ext_id) DO UPDATE SET season_no = excluded.season_no, \
                     episode_no = excluded.episode_no, title = excluded.title, \
                     overview = excluded.overview, still = excluded.still, \
                     runtime_s = excluded.runtime_s, container = excluded.container",
                )
                .bind(series_pk)
                .bind(ext_id)
                .bind(season)
                .bind(ep_no)
                .bind(&title)
                .bind(einfo.get("plot").and_then(flex_str))
                .bind(
                    ["movie_image", "cover_big"]
                        .iter()
                        .find_map(|k| einfo.get(*k).and_then(flex_str))
                        .filter(|u| u.starts_with("http")),
                )
                .bind(duration_secs(&einfo))
                .bind(e.get("container_extension").and_then(flex_str))
                .execute(&mut *tx)
                .await?;
                seen.push(ext_id);
            }
        }
        if !seen.is_empty() {
            sqlx::query(
                "DELETE FROM vod_episodes WHERE series_pk = ? \
                 AND ext_id NOT IN (SELECT value FROM json_each(?))",
            )
            .bind(series_pk)
            .bind(serde_json::to_string(&seen).unwrap_or_default())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// `"A, B / C"` → JSON `["A","B","C"]`.
fn names_json(s: &str) -> String {
    let v: Vec<&str> = s
        .split([',', '/', '|'])
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
}

/// Where to read a VOD item from.
#[derive(Debug, Clone)]
pub struct PlayTarget {
    pub source_id: i64,
    /// Full provider URL **with credentials** — never leaves the server.
    pub url: String,
    pub container: String,
    pub title: String,
    pub max_connections: Option<u32>,
    pub user_agent: Option<String>,
    /// Series: the show this episode belongs to.
    pub series_id: Option<i64>,
}

/// Resolve a movie (`kind = "movie"`) or an episode (`"episode"`) to its stream URL.
pub async fn resolve_target(pool: &SqlitePool, kind: &str, id: i64) -> Result<PlayTarget> {
    let (source_id, ext, container, title, series_id): (
        i64,
        i64,
        Option<String>,
        String,
        Option<i64>,
    ) = match kind {
        "movie" => {
            let r: Option<(i64, i64, Option<String>, String)> = sqlx::query_as(
                    "SELECT source_id, stream_id, container, title FROM vod_movies WHERE id = ? AND missing = 0",
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
            let (s, e, c, t) = r.ok_or_else(|| Error::NotFound(format!("movie {id}")))?;
            (s, e, c, t, None)
        }
        "episode" => {
            let r: Option<(i64, i64, Option<String>, String, i64)> = sqlx::query_as(
                "SELECT s.source_id, e.ext_id, e.container, s.title || ' – ' || e.title, s.id \
                     FROM vod_episodes e JOIN vod_series s ON s.id = e.series_pk WHERE e.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            let (s, e, c, t, sp) = r.ok_or_else(|| Error::NotFound(format!("episode {id}")))?;
            (s, e, c, t, Some(sp))
        }
        _ => return Err(Error::Invalid("kind must be movie or episode".into())),
    };
    let src = store::get_source(pool, source_id).await?;
    let container = container
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "mp4".into());
    target_on(&src, kind, ext, container, title, series_id)
}

/// [`resolve_target`] followed by the same title through the provider's other enabled logins
/// whose catalog has it too (#297): each one is another provider connection.
pub async fn resolve_targets(pool: &SqlitePool, kind: &str, id: i64) -> Result<Vec<PlayTarget>> {
    let first = resolve_target(pool, kind, id).await?;
    let mirrors = crate::dedup::mirrors(pool, first.source_id).await?;
    let mut out = vec![];
    for m in mirrors {
        // movie: same stream id; episode: same episode id in a series the login also has
        let ext: Option<i64> = sqlx::query_scalar(if kind == "movie" {
            "SELECT o.stream_id FROM vod_movies i JOIN vod_movies o ON o.stream_id = i.stream_id \
             WHERE i.id = ?1 AND o.source_id = ?2 AND o.missing = 0"
        } else {
            "SELECT e.ext_id FROM vod_episodes e JOIN vod_series s ON s.id = e.series_pk \
             JOIN vod_series o ON o.series_id = s.series_id \
             WHERE e.id = ?1 AND o.source_id = ?2 AND o.missing = 0"
        })
        .bind(id)
        .bind(m.id)
        .fetch_optional(pool)
        .await?;
        let Some(ext) = ext else { continue };
        if let Ok(t) = target_on(
            &m,
            kind,
            ext,
            first.container.clone(),
            first.title.clone(),
            first.series_id,
        ) {
            out.push(t);
        }
    }
    out.insert(0, first);
    Ok(out)
}

fn target_on(
    src: &Source,
    kind: &str,
    ext: i64,
    container: String,
    title: String,
    series_id: Option<i64>,
) -> Result<PlayTarget> {
    let (Some(u), Some(p)) = (&src.username, &src.password) else {
        return Err(Error::Invalid("source has no credentials".into()));
    };
    let x = XtreamClient::new(reqwest::Client::new(), &src.url, u, p);
    let url = if kind == "movie" {
        x.vod_url(ext, &container)
    } else {
        x.series_url(ext, &container)
    };
    Ok(PlayTarget {
        source_id: src.id,
        url,
        container,
        title,
        max_connections: src.max_connections.filter(|m| *m > 0),
        user_agent: src.user_agent.clone(),
        series_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cleaned_and_years_extracted() {
        assert_eq!(
            clean_title("AR| Some Movie (2019)"),
            ("Some Movie".into(), Some(2019))
        );
        assert_eq!(
            clean_title("[EN] The Show [2021]"),
            ("The Show".into(), Some(2021))
        );
        assert_eq!(clean_title("| UK | Sky Film"), ("Sky Film".into(), None));
        assert_eq!(clean_title("Plain Title"), ("Plain Title".into(), None));
        assert_eq!(clean_title("2012"), ("2012".into(), None));
        assert_eq!(
            clean_title("Blade Runner 2049 (2017)"),
            ("Blade Runner 2049".into(), Some(2017))
        );
        assert_eq!(
            clean_title("TOP GUN: Maverick"),
            ("TOP GUN: Maverick".into(), None)
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=abcDEF12345&t=3"),
            Some("abcDEF12345".into())
        );
        assert_eq!(youtube_id("abcDEF12345"), Some("abcDEF12345".into()));
        assert_eq!(
            youtube_id("https://youtu.be/abcDEF12345?x=1"),
            Some("abcDEF12345".into())
        );
        assert_eq!(youtube_id(""), None);
        assert_eq!(
            genres_of("Action / Sci-Fi, Drama"),
            Some("Action,Sci-Fi,Drama".into())
        );
        assert_eq!(
            duration_secs(&serde_json::json!({"duration": "01:30:05"})),
            Some(5405)
        );
        assert_eq!(
            duration_secs(&serde_json::json!({"duration_secs": 7200})),
            Some(7200)
        );
        assert_eq!(rating_of(&serde_json::json!({"rating": "7.5"})), Some(7.5));
        assert_eq!(rating_of(&serde_json::json!({"rating": 0})), None);
    }
}

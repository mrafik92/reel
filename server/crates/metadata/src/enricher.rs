//! TMDB (+ local `.nfo` / art) enrichment of `media_items`.
//!
//! Precedence: locked field (never touched) > local nfo/art > TMDB.
//! Locked fields are column-ish names in `locked_fields_json`, e.g.
//! `["title","overview","poster","backdrop","genres","credits"]`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use sqlx::SqlitePool;

use crate::{
    Error, Result,
    matcher::{self, MatchQuery},
    nfo::{self, Nfo},
    tmdb::{
        Credits, ExternalIds, Genre, MovieDetails, SeasonDetails, TmdbClient, TvDetails, year_of,
    },
};

const TOP_CAST: usize = 20;
const POSTER_NAMES: &[&str] = &["poster.jpg", "poster.png", "folder.jpg", "folder.png"];
const BACKDROP_NAMES: &[&str] = &["fanart.jpg", "fanart.png", "backdrop.jpg", "backdrop.png"];

#[derive(Debug, Clone)]
pub struct TmdbEnricher {
    client: TmdbClient,
}

#[derive(sqlx::FromRow, Debug, Clone)]
struct Item {
    id: i64,
    kind: String,
    parent_id: Option<i64>,
    title: String,
    sort_title: Option<String>,
    year: Option<i64>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    overview: Option<String>,
    rating: Option<f64>,
    content_rating: Option<String>,
    runtime_s: Option<i64>,
    air_date: Option<String>,
    poster: Option<String>,
    backdrop: Option<String>,
    locked_fields_json: String,
}

const ITEM_COLS: &str = "id, kind, parent_id, title, sort_title, year, tmdb_id, tvdb_id, imdb_id, overview, rating, content_rating, runtime_s, air_date, poster, backdrop, locked_fields_json";

struct Locked(HashSet<String>);
impl Locked {
    fn of(item: &Item) -> Self {
        let v: Vec<String> = serde_json::from_str(&item.locked_fields_json).unwrap_or_default();
        Locked(v.into_iter().map(|s| s.to_lowercase()).collect())
    }
    fn has(&self, f: &str) -> bool {
        self.0.contains(f)
    }
}

/// Keep `cur` when locked, else take `new` if present.
fn pick<T: Clone>(locked: bool, cur: &Option<T>, new: Option<T>) -> Option<T> {
    if locked {
        cur.clone()
    } else {
        new.or_else(|| cur.clone())
    }
}

pub fn sort_title(t: &str) -> String {
    let l = t.trim().to_lowercase();
    for a in ["the ", "an ", "a "] {
        if let Some(r) = l.strip_prefix(a) {
            if !r.is_empty() {
                return r.to_string();
            }
        }
    }
    l
}

fn find_file(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

fn file_url(p: &Path) -> String {
    format!("file://{}", p.display())
}

impl TmdbEnricher {
    pub fn new(client: TmdbClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &TmdbClient {
        &self.client
    }

    /// Enrich one item. Seasons/episodes are routed to their show.
    pub async fn enrich(&self, pool: &SqlitePool, item_id: i64) -> Result<()> {
        let mut item = load(pool, item_id)
            .await?
            .ok_or(Error::ItemNotFound(item_id))?;
        while matches!(item.kind.as_str(), "season" | "episode") {
            let Some(p) = item.parent_id else {
                return Ok(());
            };
            item = load(pool, p).await?.ok_or(Error::ItemNotFound(p))?;
        }
        match item.kind.as_str() {
            "movie" => self.enrich_movie(pool, item).await,
            "show" => self.enrich_show(pool, item).await,
            _ => Ok(()),
        }
    }

    async fn enrich_movie(&self, pool: &SqlitePool, item: Item) -> Result<()> {
        let files: Vec<(String,)> =
            sqlx::query_as("SELECT path FROM media_files WHERE item_id = ? ORDER BY part_no, id")
                .bind(item.id)
                .fetch_all(pool)
                .await?;
        let first = files.first().map(|f| PathBuf::from(&f.0));
        let dir = first
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf);
        let stem = first
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned());
        let dirs: Vec<PathBuf> = dir.iter().cloned().collect();

        let mut nfo_path = None;
        if let (Some(d), Some(s)) = (&dir, &stem) {
            nfo_path = [d.join(format!("{s}.nfo")), d.join("movie.nfo")]
                .into_iter()
                .find(|p| p.is_file());
        }
        let nfo = match nfo_path {
            Some(p) => nfo::read(&p).await.unwrap_or_default(),
            None => Nfo::default(),
        };
        let mut poster_names: Vec<String> = POSTER_NAMES.iter().map(|s| s.to_string()).collect();
        let mut back_names: Vec<String> = BACKDROP_NAMES.iter().map(|s| s.to_string()).collect();
        if let Some(s) = &stem {
            poster_names.insert(0, format!("{s}-poster.jpg"));
            back_names.insert(0, format!("{s}-fanart.jpg"));
        }
        let pn: Vec<&str> = poster_names.iter().map(String::as_str).collect();
        let bn: Vec<&str> = back_names.iter().map(String::as_str).collect();
        let local_poster = find_file(&dirs, &pn).map(|p| file_url(&p));
        let local_back = find_file(&dirs, &bn).map(|p| file_url(&p));

        let tmdb_id = match item.tmdb_id.or(nfo.tmdb_id) {
            Some(id) => Some(id),
            None => {
                let q = MatchQuery {
                    title: nfo.title.clone().unwrap_or_else(|| item.title.clone()),
                    year: nfo.year.or(item.year.map(|y| y as i32)),
                    tmdb_id: None,
                };
                matcher::resolve(&self.client, &q, false).await?
            }
        };
        let details = match tmdb_id {
            Some(id) => Some(self.client.movie(id).await?),
            None => None,
        };
        apply_movie(
            pool,
            &item,
            &nfo,
            local_poster,
            local_back,
            tmdb_id,
            details,
        )
        .await
    }

    async fn enrich_show(&self, pool: &SqlitePool, item: Item) -> Result<()> {
        let eps: Vec<(String,)> = sqlx::query_as(
            "SELECT f.path FROM media_files f JOIN media_items e ON e.id = f.item_id
             WHERE e.kind = 'episode' AND (e.parent_id = ?1
               OR e.parent_id IN (SELECT id FROM media_items WHERE parent_id = ?1 AND kind = 'season'))
             ORDER BY f.path LIMIT 1",
        )
        .bind(item.id)
        .fetch_all(pool)
        .await?;
        let mut dirs: Vec<PathBuf> = vec![];
        if let Some(p) = eps.first().map(|e| PathBuf::from(&e.0)) {
            if let Some(d) = p.parent() {
                dirs.push(d.to_path_buf());
                if let Some(g) = d.parent() {
                    dirs.push(g.to_path_buf());
                }
            }
        }
        let nfo = match find_file(&dirs, &["tvshow.nfo"]) {
            Some(p) => nfo::read(&p).await.unwrap_or_default(),
            None => Nfo::default(),
        };
        let local_poster = find_file(&dirs, POSTER_NAMES).map(|p| file_url(&p));
        let local_back = find_file(&dirs, BACKDROP_NAMES).map(|p| file_url(&p));

        let tmdb_id = match item.tmdb_id.or(nfo.tmdb_id) {
            Some(id) => Some(id),
            None => {
                let q = MatchQuery {
                    title: nfo.title.clone().unwrap_or_else(|| item.title.clone()),
                    year: nfo.year.or(item.year.map(|y| y as i32)),
                    tmdb_id: None,
                };
                matcher::resolve(&self.client, &q, true).await?
            }
        };
        let details = match tmdb_id {
            Some(id) => Some(self.client.tv(id).await?),
            None => None,
        };
        let mut seasons = vec![];
        if let (Some(id), Some(d)) = (tmdb_id, &details) {
            for s in &d.seasons {
                match self.client.season(id, s.season_number).await {
                    Ok(sd) => seasons.push(sd),
                    Err(e) => {
                        tracing::warn!(tmdb_id = id, season = s.season_number, error = %e, "season fetch failed")
                    }
                }
            }
        }
        apply_show(
            pool,
            &item,
            &nfo,
            local_poster,
            local_back,
            tmdb_id,
            details,
            seasons,
        )
        .await
    }
}

async fn load(pool: &SqlitePool, id: i64) -> Result<Option<Item>> {
    Ok(
        sqlx::query_as(&format!("SELECT {ITEM_COLS} FROM media_items WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

async fn save(pool: &SqlitePool, it: &Item) -> Result<()> {
    sqlx::query(
        "UPDATE media_items SET title=?, sort_title=?, year=?, tmdb_id=?, tvdb_id=?, imdb_id=?, overview=?,
         rating=?, content_rating=?, runtime_s=?, air_date=?, poster=?, backdrop=?, updated_at=unixepoch() WHERE id=?",
    )
    .bind(&it.title)
    .bind(&it.sort_title)
    .bind(it.year)
    .bind(it.tmdb_id)
    .bind(it.tvdb_id)
    .bind(&it.imdb_id)
    .bind(&it.overview)
    .bind(it.rating)
    .bind(&it.content_rating)
    .bind(it.runtime_s)
    .bind(&it.air_date)
    .bind(&it.poster)
    .bind(&it.backdrop)
    .bind(it.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Shared merge of top-level fields. `m` carries remote values.
struct Remote {
    title: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
    rating: Option<f64>,
    content_rating: Option<String>,
    runtime_s: Option<i64>,
    air_date: Option<String>,
    poster: Option<String>,
    backdrop: Option<String>,
    tmdb_id: Option<i64>,
    ext: ExternalIds,
    imdb_id: Option<String>,
}

fn merge(item: &Item, nfo: &Nfo, lp: Option<String>, lb: Option<String>, r: Remote) -> Item {
    let lk = Locked::of(item);
    let mut o = item.clone();
    let title = pick(
        lk.has("title"),
        &Some(item.title.clone()),
        nfo.title.clone().or(r.title),
    );
    o.title = title.unwrap_or_else(|| item.title.clone());
    o.sort_title = if lk.has("sort_title") {
        item.sort_title.clone()
    } else {
        Some(sort_title(&o.title))
    };
    o.year = pick(
        lk.has("year"),
        &item.year,
        nfo.year.map(i64::from).or(r.year),
    );
    o.overview = pick(
        lk.has("overview"),
        &item.overview,
        nfo.plot.clone().or(r.overview),
    );
    o.rating = pick(lk.has("rating"), &item.rating, r.rating);
    o.content_rating = pick(
        lk.has("content_rating"),
        &item.content_rating,
        r.content_rating,
    );
    o.runtime_s = pick(lk.has("runtime"), &item.runtime_s, r.runtime_s);
    o.air_date = pick(lk.has("air_date"), &item.air_date, r.air_date);
    o.poster = pick(lk.has("poster"), &item.poster, lp.or(r.poster));
    o.backdrop = pick(lk.has("backdrop"), &item.backdrop, lb.or(r.backdrop));
    o.tmdb_id = r.tmdb_id.or(nfo.tmdb_id).or(item.tmdb_id);
    o.imdb_id = r
        .imdb_id
        .or_else(|| r.ext.imdb_id.clone())
        .or_else(|| nfo.imdb_id.clone())
        .or_else(|| item.imdb_id.clone());
    o.tvdb_id = r.ext.tvdb_id.or(nfo.tvdb_id).or(item.tvdb_id);
    o
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

async fn apply_movie(
    pool: &SqlitePool,
    item: &Item,
    nfo: &Nfo,
    lp: Option<String>,
    lb: Option<String>,
    tmdb_id: Option<i64>,
    d: Option<MovieDetails>,
) -> Result<()> {
    let lk = Locked::of(item);
    let remote = match &d {
        Some(d) => Remote {
            title: Some(d.title.clone()),
            year: year_of(d.release_date.as_deref()).map(i64::from),
            overview: nonempty(d.overview.clone()),
            rating: d.vote_average,
            content_rating: d.us_certification(),
            runtime_s: d.runtime.filter(|m| *m > 0).map(|m| m * 60),
            air_date: nonempty(d.release_date.clone()),
            poster: d.poster_path.clone(),
            backdrop: d.backdrop_path.clone(),
            tmdb_id,
            ext: d.external_ids.clone().unwrap_or_default(),
            imdb_id: nonempty(d.imdb_id.clone()),
        },
        None => Remote {
            title: None,
            year: None,
            overview: None,
            rating: None,
            content_rating: None,
            runtime_s: None,
            air_date: None,
            poster: None,
            backdrop: None,
            tmdb_id: None,
            ext: ExternalIds::default(),
            imdb_id: None,
        },
    };
    let merged = merge(item, nfo, lp, lb, remote);
    save(pool, &merged).await?;
    if let Some(d) = d {
        if !lk.has("genres") {
            set_genres(pool, item.id, &d.genres).await?;
        }
        if !lk.has("credits") {
            if let Some(c) = &d.credits {
                set_credits(pool, item.id, c, true).await?;
            }
        }
        if let Some(col) = &d.belongs_to_collection {
            let (cid,): (i64,) = sqlx::query_as(
                "INSERT INTO collections (name, tmdb_id) VALUES (?, ?)
                 ON CONFLICT(tmdb_id) DO UPDATE SET name = excluded.name RETURNING id",
            )
            .bind(&col.name)
            .bind(col.id)
            .fetch_one(pool)
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO collection_items (collection_id, item_id) VALUES (?, ?)",
            )
            .bind(cid)
            .bind(item.id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_show(
    pool: &SqlitePool,
    item: &Item,
    nfo: &Nfo,
    lp: Option<String>,
    lb: Option<String>,
    tmdb_id: Option<i64>,
    d: Option<TvDetails>,
    seasons: Vec<SeasonDetails>,
) -> Result<()> {
    let lk = Locked::of(item);
    let remote = match &d {
        Some(d) => Remote {
            title: Some(d.name.clone()),
            year: year_of(d.first_air_date.as_deref()).map(i64::from),
            overview: nonempty(d.overview.clone()),
            rating: d.vote_average,
            content_rating: d.us_rating(),
            runtime_s: d
                .episode_run_time
                .first()
                .filter(|m| **m > 0)
                .map(|m| m * 60),
            air_date: nonempty(d.first_air_date.clone()),
            poster: d.poster_path.clone(),
            backdrop: d.backdrop_path.clone(),
            tmdb_id,
            ext: d.external_ids.clone().unwrap_or_default(),
            imdb_id: None,
        },
        None => Remote {
            title: None,
            year: None,
            overview: None,
            rating: None,
            content_rating: None,
            runtime_s: None,
            air_date: None,
            poster: None,
            backdrop: None,
            tmdb_id: None,
            ext: ExternalIds::default(),
            imdb_id: None,
        },
    };
    let merged = merge(item, nfo, lp, lb, remote);
    save(pool, &merged).await?;
    let Some(d) = d else { return Ok(()) };
    if !lk.has("genres") {
        set_genres(pool, item.id, &d.genres).await?;
    }
    if !lk.has("credits") {
        if let Some(c) = &d.credits {
            set_credits(pool, item.id, c, false).await?;
        }
    }
    for s in seasons {
        apply_season(pool, item, &s).await?;
    }
    Ok(())
}

/// Update existing season/episode rows only (never creates rows for files we don't have).
async fn apply_season(pool: &SqlitePool, show: &Item, s: &SeasonDetails) -> Result<()> {
    let season: Option<Item> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLS} FROM media_items WHERE kind='season' AND parent_id=? AND season_no=?"
    ))
    .bind(show.id)
    .bind(s.season_number)
    .fetch_optional(pool)
    .await?;
    if let Some(it) = season {
        let lk = Locked::of(&it);
        let mut o = it.clone();
        if !lk.has("title") {
            if let Some(n) = nonempty(s.name.clone()) {
                o.title = n;
            }
        }
        o.overview = pick(
            lk.has("overview"),
            &it.overview,
            nonempty(s.overview.clone()),
        );
        o.air_date = pick(
            lk.has("air_date"),
            &it.air_date,
            nonempty(s.air_date.clone()),
        );
        o.poster = pick(lk.has("poster"), &it.poster, s.poster_path.clone());
        save(pool, &o).await?;
    }
    let eps: Vec<Item> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLS} FROM media_items WHERE kind='episode' AND season_no=?2
         AND (parent_id=?1 OR parent_id IN (SELECT id FROM media_items WHERE parent_id=?1 AND kind='season'))"
    ))
    .bind(show.id)
    .bind(s.season_number)
    .fetch_all(pool)
    .await?;
    for it in eps {
        let Some(no) = episode_no(pool, it.id).await? else {
            continue;
        };
        let Some(e) = s.episodes.iter().find(|e| e.episode_number == no) else {
            continue;
        };
        let lk = Locked::of(&it);
        let mut o = it.clone();
        if !lk.has("title") {
            if let Some(n) = nonempty(e.name.clone()) {
                o.title = n;
            }
        }
        o.overview = pick(
            lk.has("overview"),
            &it.overview,
            nonempty(e.overview.clone()),
        );
        o.air_date = pick(
            lk.has("air_date"),
            &it.air_date,
            nonempty(e.air_date.clone()),
        );
        o.poster = pick(lk.has("poster"), &it.poster, e.still_path.clone());
        o.rating = pick(lk.has("rating"), &it.rating, e.vote_average);
        o.runtime_s = pick(
            lk.has("runtime"),
            &it.runtime_s,
            e.runtime.filter(|m| *m > 0).map(|m| m * 60),
        );
        save(pool, &o).await?;
    }
    Ok(())
}

async fn episode_no(pool: &SqlitePool, id: i64) -> Result<Option<i64>> {
    let r: Option<(Option<i64>,)> = sqlx::query_as("SELECT episode_no FROM media_items WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(r.and_then(|r| r.0))
}

async fn set_genres(pool: &SqlitePool, item_id: i64, genres: &[Genre]) -> Result<()> {
    if genres.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM item_genres WHERE item_id=?")
        .bind(item_id)
        .execute(pool)
        .await?;
    for g in genres {
        sqlx::query("INSERT OR IGNORE INTO genres (name) VALUES (?)")
            .bind(&g.name)
            .execute(pool)
            .await?;
        sqlx::query("INSERT OR IGNORE INTO item_genres (item_id, genre_id) SELECT ?, id FROM genres WHERE name=?")
            .bind(item_id)
            .bind(&g.name)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn upsert_person(
    pool: &SqlitePool,
    tmdb_id: i64,
    name: &str,
    photo: &Option<String>,
) -> Result<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO people (tmdb_id, name, photo) VALUES (?, ?, ?)
         ON CONFLICT(tmdb_id) DO UPDATE SET name = excluded.name, photo = COALESCE(excluded.photo, people.photo)
         RETURNING id",
    )
    .bind(tmdb_id)
    .bind(name)
    .bind(photo)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn set_credits(
    pool: &SqlitePool,
    item_id: i64,
    c: &Credits,
    with_directors: bool,
) -> Result<()> {
    if c.cast.is_empty() && c.crew.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM credits WHERE item_id=?")
        .bind(item_id)
        .execute(pool)
        .await?;
    let mut cast: Vec<_> = c.cast.iter().collect();
    cast.sort_by_key(|m| m.order);
    for m in cast.into_iter().take(TOP_CAST) {
        let pid = upsert_person(pool, m.id, &m.name, &m.profile_path).await?;
        sqlx::query("INSERT OR IGNORE INTO credits (item_id, person_id, role, character, ord) VALUES (?, ?, 'actor', ?, ?)")
            .bind(item_id)
            .bind(pid)
            .bind(&m.character)
            .bind(m.order)
            .execute(pool)
            .await?;
    }
    if with_directors {
        for (i, m) in c
            .crew
            .iter()
            .filter(|m| m.job.as_deref() == Some("Director"))
            .enumerate()
        {
            let pid = upsert_person(pool, m.id, &m.name, &m.profile_path).await?;
            sqlx::query("INSERT OR IGNORE INTO credits (item_id, person_id, role, character, ord) VALUES (?, ?, 'director', NULL, ?)")
                .bind(item_id)
                .bind(pid)
                .bind(i as i64)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

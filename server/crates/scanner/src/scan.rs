//! Scan engine: walk library paths, incremental by (path,size,mtime), probe, persist.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use reel_core::{Error, Result, ServerEvent};
use sqlx::{Row, SqliteConnection, SqlitePool};
use tokio::{sync::broadcast, task::JoinSet};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::{
    parser::{
        FileClass, LibraryKind, MediaKind, ParsedMedia, SidecarSubtitle, classify, extension,
        is_ignored_dir, is_subtitle_ext, is_video_ext,
    },
    probe::{self, ProbeResult},
};

/// Hook for metadata enrichment (e.g. TMDB). Called for every newly created
/// `media_items` row (show, season, episode, movie) after a scan's DB writes.
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// Enrich `item_id`. Default: no-op.
    async fn enrich(&self, _item_id: i64) -> Result<()> {
        Ok(())
    }
}

/// Counters returned from a scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Video files considered.
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub removed: u64,
    /// Files recognised as renamed/moved (same size, duration and stream layout) and re-pointed
    /// instead of deleted + re-added; item, metadata and watch state are preserved.
    pub moved: u64,
    /// Files that failed to probe (retried on next scan).
    pub failed: u64,
}

/// The library scanner. Cheap to clone.
#[derive(Clone)]
pub struct Scanner {
    events: Option<broadcast::Sender<ServerEvent>>,
    metadata: Option<Arc<dyn MetadataProvider>>,
    ffprobe: String,
    probe_timeout: Duration,
    concurrency: usize,
    lock: Arc<tokio::sync::Mutex<()>>,
    /// Files that vanished recently, kept so a later "new file" event can claim them (rename
    /// detection across separate watcher events).
    tombs: Arc<Mutex<Vec<Tomb>>>,
    rename_window: Duration,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            events: None,
            metadata: None,
            ffprobe: "ffprobe".into(),
            probe_timeout: Duration::from_secs(60),
            concurrency: 4,
            lock: Arc::default(),
            tombs: Arc::default(),
            rename_window: Duration::ZERO,
        }
    }

    /// Emit `ServerEvent::ScanProgress` on this channel.
    pub fn with_events(mut self, tx: broadcast::Sender<ServerEvent>) -> Self {
        self.events = Some(tx);
        self
    }

    pub fn with_metadata(mut self, m: Arc<dyn MetadataProvider>) -> Self {
        self.metadata = Some(m);
        self
    }

    pub fn with_ffprobe_path(mut self, p: impl Into<String>) -> Self {
        self.ffprobe = p.into();
        self
    }

    pub fn with_probe_timeout(mut self, d: Duration) -> Self {
        self.probe_timeout = d;
        self
    }

    /// How long a file removed by a *targeted* rescan (watcher event) stays claimable by a new file
    /// with the same size, duration and stream layout, so a rename delivered as two separate events
    /// keeps its item. Orphaned items are removed once the window has passed. Default: zero (items
    /// are removed immediately); renames within a single scan are always detected.
    pub fn with_rename_window(mut self, d: Duration) -> Self {
        self.rename_window = d;
        self
    }

    /// Max concurrent ffprobe processes (min 1).
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    /// Full (incremental) scan of every path of a library.
    pub async fn scan_library(&self, pool: &SqlitePool, library_id: i64) -> Result<ScanSummary> {
        let lib = load_library(pool, library_id).await?;
        let scopes = lib
            .roots
            .iter()
            .map(|r| Scope {
                path: r.clone(),
                max_depth: None,
            })
            .collect();
        self.scan_scopes(pool, &lib, scopes, false).await
    }

    /// Targeted rescan of a file or directory (also handles deletions).
    /// Errors with `NotFound` if no library contains `path`.
    pub async fn scan_path(&self, pool: &SqlitePool, path: &Path) -> Result<ScanSummary> {
        let libs = load_libraries(pool).await?;
        let lib = libs
            .into_iter()
            .filter_map(|l| {
                let depth = l
                    .roots
                    .iter()
                    .filter(|r| path.starts_with(r))
                    .map(|r| r.components().count())
                    .max()?;
                Some((depth, l))
            })
            .max_by_key(|(d, _)| *d)
            .map(|(_, l)| l)
            .ok_or(Error::NotFound)?;
        let mut scopes = vec![Scope {
            path: path.to_path_buf(),
            max_depth: None,
        }];
        if let (false, Some(parent)) = (path.is_dir(), path.parent()) {
            // File (or deleted path): also refresh siblings (sidecars) non-recursively.
            scopes.push(Scope {
                path: parent.to_path_buf(),
                max_depth: Some(1),
            });
        }
        // Targeted rescans come from watcher events: a vanished file may be half of a rename whose
        // other half arrives as a separate event, so removal of its item is deferred briefly.
        self.scan_scopes(pool, &lib, scopes, true).await
    }

    fn emit(&self, library_id: i64, scanned: u64, total: u64) {
        if let Some(tx) = &self.events {
            let _ = tx.send(ServerEvent::ScanProgress {
                library_id,
                scanned,
                total,
            });
        }
    }

    async fn scan_scopes(
        &self,
        pool: &SqlitePool,
        lib: &Library,
        scopes: Vec<Scope>,
        defer_removals: bool,
    ) -> Result<ScanSummary> {
        let _guard = self.lock.lock().await;
        let mut summary = ScanSummary::default();

        // Walk. Skip scopes whose library root is unavailable (e.g. unmounted) so we
        // never mass-delete rows because of a missing mount.
        let mut active: Vec<Scope> = Vec::new();
        let mut entries: HashMap<String, Entry> = HashMap::new();
        for scope in scopes {
            let Some(root) = lib.roots.iter().find(|r| scope.path.starts_with(r)) else {
                continue;
            };
            if !root.is_dir() {
                warn!(root = %root.display(), "library root unavailable; skipping");
                continue;
            }
            let s = scope.clone();
            let found = tokio::task::spawn_blocking(move || walk(&s))
                .await
                .map_err(|e| Error::Other(e.to_string()))?;
            for e in found {
                entries.insert(e.path_str.clone(), e);
            }
            active.push(scope);
        }

        // Classify.
        let mut videos: Vec<(Entry, ParsedMedia)> = Vec::new();
        let mut subs: HashMap<(PathBuf, String), Vec<SubFile>> = HashMap::new();
        let mut sorted: Vec<&Entry> = entries.values().collect();
        sorted.sort_by(|a, b| a.path_str.cmp(&b.path_str));
        for e in sorted {
            let Some(root) = lib.roots.iter().find(|r| e.path.starts_with(r)) else {
                continue;
            };
            let rel = e.path.strip_prefix(root).unwrap_or(&e.path);
            let Some(rel) = rel.to_str() else { continue };
            match classify(rel, Some(lib.kind)) {
                FileClass::Video(p) => videos.push((e.clone(), p)),
                FileClass::Subtitle(s) => {
                    let dir = e.path.parent().map(Path::to_path_buf).unwrap_or_default();
                    let sf = SubFile {
                        path: e.path_str.clone(),
                        info: s.clone(),
                    };
                    subs.entry((dir.clone(), s.base_stem.clone()))
                        .or_default()
                        .push(sf.clone());
                    if s.base_stem != s.full_stem {
                        let mut alt = sf;
                        alt.info.language = None;
                        alt.info.forced = false;
                        alt.info.default = false;
                        alt.info.sdh = false;
                        subs.entry((dir, s.full_stem.clone()))
                            .or_default()
                            .push(alt);
                    }
                }
                FileClass::Ignored => {}
            }
        }

        let existing = load_existing(pool, lib.id).await?;
        let total = videos.len() as u64;
        self.emit(lib.id, 0, total);

        let mut created: Vec<i64> = Vec::new();
        let mut queue: VecDeque<Job> = VecDeque::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut done = 0u64;

        for (entry, parsed) in videos {
            seen.insert(entry.path_str.clone());
            let file_subs = subs_for(&subs, &entry);
            let ex = existing.get(&entry.path_str).cloned();
            if let Some(ex) = ex
                .as_ref()
                .filter(|ex| ex.size == entry.size && ex.mtime == entry.mtime)
            {
                refresh_sidecars(pool, ex.id, &file_subs).await?;
                ensure_links(pool, lib, ex.id, &parsed, &mut created).await?;
                summary.unchanged += 1;
                summary.scanned += 1;
                done += 1;
                if done % 50 == 0 {
                    self.emit(lib.id, done, total);
                }
                continue;
            }
            queue.push_back(Job {
                entry,
                parsed,
                existing: ex,
                subs: file_subs,
            });
        }

        // Rename detection: rows whose file was not seen in this walk, by size.
        let mut vanished_by_size: HashMap<i64, Vec<(String, ExistingFile)>> = HashMap::new();
        if queue.iter().any(|j| j.existing.is_none()) {
            for (path, ex) in &existing {
                if !seen.contains(path) {
                    vanished_by_size
                        .entry(ex.size)
                        .or_default()
                        .push((path.clone(), ex.clone()));
                }
            }
        }
        let mut moved_ids: HashSet<i64> = HashSet::new();

        // Probe with bounded concurrency; DB writes happen serially here.
        let mut set: JoinSet<(Job, std::result::Result<ProbeResult, probe::ProbeError>)> =
            JoinSet::new();
        loop {
            while set.len() < self.concurrency {
                let Some(job) = queue.pop_front() else { break };
                let ffprobe = self.ffprobe.clone();
                let timeout = self.probe_timeout;
                set.spawn(async move {
                    let r = probe::probe(&ffprobe, &job.entry.path, timeout).await;
                    (job, r)
                });
            }
            let Some(joined) = set.join_next().await else {
                break;
            };
            let (mut job, res) = match joined {
                Ok(v) => v,
                Err(e) => {
                    warn!("probe task failed: {e}");
                    summary.failed += 1;
                    summary.scanned += 1;
                    done += 1;
                    continue;
                }
            };
            summary.scanned += 1;
            done += 1;
            match res {
                Ok(pr) => {
                    let mut forced: Option<(i64, Vec<i64>)> = None;
                    if job.existing.is_none() {
                        match self
                            .find_moved(pool, lib.id, &job, &pr, &vanished_by_size, &moved_ids)
                            .await
                        {
                            Ok(Some(Moved::Row(ex))) => {
                                debug!(from = ex.id, to = %job.entry.path_str, "file moved/renamed");
                                moved_ids.insert(ex.id);
                                job.existing = Some(ex);
                                summary.moved += 1;
                            }
                            Ok(Some(Moved::Tomb { item_id, links })) => {
                                debug!(item = item_id, to = %job.entry.path_str, "file moved/renamed (after removal)");
                                forced = Some((item_id, links));
                                summary.moved += 1;
                            }
                            Ok(None) => {}
                            Err(e) => warn!("rename detection failed: {e}"),
                        }
                    }
                    let was_move = forced.is_some()
                        || job
                            .existing
                            .as_ref()
                            .is_some_and(|e| moved_ids.contains(&e.id));
                    match store_file(pool, lib, &job, &pr, &mut created, forced).await {
                        Ok(is_new) => {
                            if was_move {
                                // counted in `moved`
                            } else if is_new {
                                summary.added += 1;
                            } else {
                                summary.updated += 1;
                            }
                        }
                        Err(e) => {
                            warn!(path = %job.entry.path_str, "failed to store file: {e}");
                            summary.failed += 1;
                        }
                    }
                }
                Err(e) => {
                    warn!(path = %job.entry.path_str, "probe failed: {e}");
                    summary.failed += 1;
                }
            }
            if done % 5 == 0 || done == total {
                self.emit(lib.id, done, total);
            }
        }

        // Vanished files: existing rows within an active scope that were not seen.
        let mut removed = false;
        let mut keep_items: Vec<i64> = Vec::new();
        for (path, ex) in &existing {
            if seen.contains(path) || moved_ids.contains(&ex.id) {
                continue;
            }
            let p = Path::new(path);
            if active.iter().any(|s| s.contains(p)) {
                // Not seen as a video: gone, or no longer classified as one. Remember it briefly
                // so a "new file" event for the same content can claim the item.
                if let Some(t) = if defer_removals && !self.rename_window.is_zero() {
                    tomb_of(pool, lib.id, ex, path).await?
                } else {
                    None
                } {
                    keep_items.push(t.item_id);
                    keep_items.extend(t.links.iter().copied());
                    self.tombs.lock().unwrap().push(t);
                }
                sqlx::query("DELETE FROM media_files WHERE id = ?")
                    .bind(ex.id)
                    .execute(pool)
                    .await?;
                summary.removed += 1;
                removed = true;
            }
        }
        if removed {
            cleanup_orphans(pool, lib.id, &keep_items).await?;
            if !keep_items.is_empty() {
                // Items waiting for a possible rename are removed once the window has passed.
                let (pool2, lib_id, wait) = (pool.clone(), lib.id, self.rename_window);
                tokio::spawn(async move {
                    tokio::time::sleep(wait + Duration::from_millis(250)).await;
                    if let Err(e) = cleanup_orphans(&pool2, lib_id, &[]).await {
                        warn!("delayed orphan cleanup failed: {e}");
                    }
                });
            }
        }

        self.emit(lib.id, total, total);
        info!(library = lib.id, ?summary, "scan complete");

        if let Some(m) = &self.metadata {
            for id in created {
                if let Err(e) = m.enrich(id).await {
                    warn!(item = id, "metadata enrich failed: {e}");
                }
            }
        }
        Ok(summary)
    }
}

// ------------------------------------------------------------------ types

struct Library {
    id: i64,
    kind: LibraryKind,
    roots: Vec<PathBuf>,
}

#[derive(Clone)]
struct Scope {
    path: PathBuf,
    max_depth: Option<usize>,
}

impl Scope {
    fn contains(&self, p: &Path) -> bool {
        match self.max_depth {
            None => p.starts_with(&self.path),
            Some(_) => p.parent() == Some(self.path.as_path()),
        }
    }
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    path_str: String,
    size: i64,
    mtime: i64,
}

#[derive(Clone)]
struct SubFile {
    path: String,
    info: SidecarSubtitle,
}

#[derive(Clone)]
struct ExistingFile {
    id: i64,
    item_id: i64,
    size: i64,
    mtime: i64,
}

/// A recently vanished file (see [`Scanner::with_rename_window`]).
struct Tomb {
    library_id: i64,
    item_id: i64,
    links: Vec<i64>,
    size: i64,
    duration: Option<f64>,
    fingerprint: String,
    stem: String,
    at: Instant,
}

struct Job {
    entry: Entry,
    parsed: ParsedMedia,
    existing: Option<ExistingFile>,
    subs: Vec<SubFile>,
}

fn subs_for(subs: &HashMap<(PathBuf, String), Vec<SubFile>>, e: &Entry) -> Vec<SubFile> {
    let dir = e.path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = e
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let mut v = subs.get(&(dir, stem)).cloned().unwrap_or_default();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v.dedup_by(|a, b| a.path == b.path);
    v
}

fn walk(scope: &Scope) -> Vec<Entry> {
    let mut wd = WalkDir::new(&scope.path).follow_links(true);
    if let Some(d) = scope.max_depth {
        wd = wd.max_depth(d);
    }
    let mut out = Vec::new();
    let it = wd.into_iter().filter_entry(|e| {
        e.depth() == 0
            || !(e.file_type().is_dir() && is_ignored_dir(&e.file_name().to_string_lossy()))
    });
    for e in it {
        let Ok(e) = e else { continue };
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Some(ext) = extension(&name) else {
            continue;
        };
        if !(is_video_ext(&ext) || is_subtitle_ext(&ext)) {
            continue;
        }
        let Some(path_str) = e.path().to_str().map(str::to_string) else {
            warn!(path = %e.path().display(), "skipping non-UTF-8 path");
            continue;
        };
        let Ok(md) = e.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry {
            path: e.into_path(),
            path_str,
            size: md.len() as i64,
            mtime,
        });
    }
    out
}

// ------------------------------------------------------------------ DB

fn parse_roots(json: &str) -> Vec<PathBuf> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

async fn load_libraries(pool: &SqlitePool) -> Result<Vec<Library>> {
    let rows = sqlx::query("SELECT id, kind, paths_json FROM libraries")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let kind = LibraryKind::from_db(r.get::<String, _>("kind").as_str())?;
            Some(Library {
                id: r.get("id"),
                kind,
                roots: parse_roots(&r.get::<String, _>("paths_json")),
            })
        })
        .collect())
}

async fn load_library(pool: &SqlitePool, id: i64) -> Result<Library> {
    load_libraries(pool)
        .await?
        .into_iter()
        .find(|l| l.id == id)
        .ok_or(Error::NotFound)
}

/// All library root paths (used by the watcher).
pub async fn library_roots(pool: &SqlitePool) -> Result<Vec<PathBuf>> {
    Ok(load_libraries(pool)
        .await?
        .into_iter()
        .flat_map(|l| l.roots)
        .collect())
}

async fn load_existing(pool: &SqlitePool, lib: i64) -> Result<HashMap<String, ExistingFile>> {
    let rows = sqlx::query(
        "SELECT f.id, f.item_id, f.path, f.size, f.mtime FROM media_files f \
         JOIN media_items i ON i.id = f.item_id WHERE i.library_id = ?",
    )
    .bind(lib)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("path"),
                ExistingFile {
                    id: r.get("id"),
                    item_id: r.get("item_id"),
                    size: r.get("size"),
                    mtime: r.get("mtime"),
                },
            )
        })
        .collect())
}

/// Remove items left without files (or children). `keep` items are spared (pending renames).
async fn cleanup_orphans(pool: &SqlitePool, lib: i64, keep: &[i64]) -> Result<()> {
    let keep = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
    for sql in [
        "DELETE FROM media_items WHERE library_id = ?1 AND kind IN ('movie','episode') \
         AND NOT EXISTS (SELECT 1 FROM media_files f WHERE f.item_id = media_items.id) \
         AND NOT EXISTS (SELECT 1 FROM media_file_links l WHERE l.item_id = media_items.id) \
         AND id NOT IN (SELECT value FROM json_each(?2))",
        "DELETE FROM media_items WHERE library_id = ?1 AND kind = 'season' \
         AND NOT EXISTS (SELECT 1 FROM media_items c WHERE c.parent_id = media_items.id)",
        "DELETE FROM media_items WHERE library_id = ?1 AND kind = 'show' \
         AND NOT EXISTS (SELECT 1 FROM media_items c WHERE c.parent_id = media_items.id)",
    ] {
        let mut q = sqlx::query(sql).bind(lib);
        if sql.contains("?2") {
            q = q.bind(&keep);
        }
        q.execute(pool).await?;
    }
    Ok(())
}

fn sort_title(t: &str) -> String {
    let l = t.to_lowercase();
    for a in ["the ", "an ", "a "] {
        if let Some(rest) = l.strip_prefix(a) {
            return rest.to_string();
        }
    }
    l
}

#[allow(clippy::too_many_arguments)]
async fn insert_item(
    conn: &mut SqliteConnection,
    lib: i64,
    kind: &str,
    parent: Option<i64>,
    title: &str,
    year: Option<i32>,
    tmdb: Option<i64>,
    season_no: Option<i32>,
    episode_no: Option<i32>,
    air_date: Option<&str>,
) -> Result<i64> {
    let r = sqlx::query(
        "INSERT INTO media_items (library_id, kind, parent_id, title, sort_title, year, tmdb_id, \
         season_no, episode_no, air_date) VALUES (?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(lib)
    .bind(kind)
    .bind(parent)
    .bind(title)
    .bind(sort_title(title))
    .bind(year)
    .bind(tmdb)
    .bind(season_no)
    .bind(episode_no)
    .bind(air_date)
    .execute(conn)
    .await?;
    Ok(r.last_insert_rowid())
}

async fn first_id<'a>(
    conn: &mut SqliteConnection,
    q: sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>,
) -> Result<Option<i64>> {
    Ok(q.fetch_optional(conn).await?.map(|r| r.get::<i64, _>(0)))
}

async fn ensure_movie(
    conn: &mut SqliteConnection,
    lib: i64,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<i64> {
    if let Some(t) = p.tmdb_id {
        let q = sqlx::query(
            "SELECT id FROM media_items WHERE library_id=? AND kind='movie' AND tmdb_id=? LIMIT 1",
        )
        .bind(lib)
        .bind(t);
        if let Some(id) = first_id(conn, q).await? {
            return Ok(id);
        }
    }
    if let Some(id) = first_id(
        conn,
        sqlx::query(
            "SELECT id FROM media_items WHERE library_id=? AND kind='movie' \
             AND title = ? COLLATE NOCASE AND year IS ? LIMIT 1",
        )
        .bind(lib)
        .bind(&p.title)
        .bind(p.year),
    )
    .await?
    {
        return Ok(id);
    }
    let id = insert_item(
        conn, lib, "movie", None, &p.title, p.year, p.tmdb_id, None, None, None,
    )
    .await?;
    created.push(id);
    Ok(id)
}

async fn ensure_show(
    conn: &mut SqliteConnection,
    lib: i64,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<i64> {
    if let Some(t) = p.tmdb_id {
        let q = sqlx::query(
            "SELECT id FROM media_items WHERE library_id=? AND kind='show' AND tmdb_id=? LIMIT 1",
        )
        .bind(lib)
        .bind(t);
        if let Some(id) = first_id(conn, q).await? {
            return Ok(id);
        }
    }
    if let Some(id) = first_id(
        conn,
        sqlx::query(
            "SELECT id FROM media_items WHERE library_id=? AND kind='show' \
             AND title = ? COLLATE NOCASE AND (? IS NULL OR year IS NULL OR year = ?) LIMIT 1",
        )
        .bind(lib)
        .bind(&p.title)
        .bind(p.year)
        .bind(p.year),
    )
    .await?
    {
        return Ok(id);
    }
    let id = insert_item(
        conn, lib, "show", None, &p.title, p.year, p.tmdb_id, None, None, None,
    )
    .await?;
    created.push(id);
    Ok(id)
}

async fn ensure_season(
    conn: &mut SqliteConnection,
    lib: i64,
    show: i64,
    no: i32,
    created: &mut Vec<i64>,
) -> Result<i64> {
    if let Some(id) = first_id(
        conn,
        sqlx::query(
            "SELECT id FROM media_items WHERE parent_id=? AND kind='season' AND season_no=? LIMIT 1",
        )
        .bind(show)
        .bind(no),
    )
    .await?
    {
        return Ok(id);
    }
    let title = if no == 0 {
        "Specials".to_string()
    } else {
        format!("Season {no}")
    };
    let id = insert_item(
        conn,
        lib,
        "season",
        Some(show),
        &title,
        None,
        None,
        Some(no),
        None,
        None,
    )
    .await?;
    created.push(id);
    Ok(id)
}

async fn ensure_episode(
    conn: &mut SqliteConnection,
    lib: i64,
    season: i64,
    season_no: i32,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<i64> {
    let first = p.episodes.first().copied();
    let found = if let Some(d) = &p.air_date {
        first_id(
            conn,
            sqlx::query(
                "SELECT id FROM media_items WHERE parent_id=? AND kind='episode' AND air_date=? LIMIT 1",
            )
            .bind(season)
            .bind(d),
        )
        .await?
    } else {
        first_id(
            conn,
            sqlx::query(
                "SELECT id FROM media_items WHERE parent_id=? AND kind='episode' AND episode_no=? LIMIT 1",
            )
            .bind(season)
            .bind(first),
        )
        .await?
    };
    if let Some(id) = found {
        return Ok(id);
    }
    let title = p
        .episode_title
        .clone()
        .unwrap_or_else(|| match (&p.air_date, first) {
            (Some(d), _) => d.clone(),
            (None, Some(n)) => format!("Episode {n}"),
            _ => "Episode".into(),
        });
    let id = insert_item(
        conn,
        lib,
        "episode",
        Some(season),
        &title,
        None,
        None,
        Some(season_no),
        first,
        p.air_date.as_deref(),
    )
    .await?;
    created.push(id);
    Ok(id)
}

async fn ensure_item(
    conn: &mut SqliteConnection,
    lib: i64,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<i64> {
    match p.kind {
        MediaKind::Movie => ensure_movie(conn, lib, p, created).await,
        MediaKind::Episode => {
            let show = ensure_show(conn, lib, p, created).await?;
            let sn = p.season.unwrap_or(1);
            let season = ensure_season(conn, lib, show, sn, created).await?;
            ensure_episode(conn, lib, season, sn, p, created).await
        }
    }
}

async fn insert_stream(
    conn: &mut SqliteConnection,
    file_id: i64,
    s: &probe::StreamInfo,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO streams (file_id, idx, kind, codec, profile, level, width, height, hdr, \
         bit_depth, channels, language, title, \"default\", forced) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(file_id)
    .bind(s.idx)
    .bind(s.kind.as_str())
    .bind(&s.codec)
    .bind(&s.profile)
    .bind(s.level)
    .bind(s.width)
    .bind(s.height)
    .bind(s.hdr.as_str())
    .bind(s.bit_depth)
    .bind(s.channels)
    .bind(&s.language)
    .bind(&s.title)
    .bind(s.default)
    .bind(s.forced)
    .execute(conn)
    .await?;
    Ok(())
}

async fn write_sidecars(conn: &mut SqliteConnection, file_id: i64, subs: &[SubFile]) -> Result<()> {
    let (max,): (i64,) =
        sqlx::query_as("SELECT COALESCE(MAX(idx), -1) FROM streams WHERE file_id = ?")
            .bind(file_id)
            .fetch_one(&mut *conn)
            .await?;
    for (n, s) in subs.iter().enumerate() {
        let name = Path::new(&s.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        sqlx::query(
            "INSERT INTO streams (file_id, idx, kind, codec, language, title, \"default\", forced, \
             external_path) VALUES (?,?,'subtitle',?,?,?,?,?,?)",
        )
        .bind(file_id)
        .bind(max + 1 + n as i64)
        .bind(&s.info.ext)
        .bind(&s.info.language)
        .bind(name)
        .bind(s.info.default)
        .bind(s.info.forced)
        .bind(&s.path)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn refresh_sidecars(pool: &SqlitePool, file_id: i64, subs: &[SubFile]) -> Result<()> {
    let have: Vec<String> = sqlx::query_scalar(
        "SELECT external_path FROM streams WHERE file_id = ? AND external_path IS NOT NULL \
         ORDER BY external_path",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    let want: Vec<&str> = subs.iter().map(|s| s.path.as_str()).collect();
    if have.iter().map(String::as_str).eq(want.iter().copied()) {
        return Ok(());
    }
    debug!(file_id, "sidecar subtitles changed");
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM streams WHERE file_id = ? AND external_path IS NOT NULL")
        .bind(file_id)
        .execute(&mut *tx)
        .await?;
    write_sidecars(&mut tx, file_id, subs).await?;
    tx.commit().await?;
    Ok(())
}

/// Returns true if a new file row was inserted.
async fn store_file(
    pool: &SqlitePool,
    lib: &Library,
    job: &Job,
    pr: &ProbeResult,
    created: &mut Vec<i64>,
    forced: Option<(i64, Vec<i64>)>,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let mut new_created = Vec::new();
    let (file_id, is_new) = match &job.existing {
        Some(ex) => {
            sqlx::query(
                "UPDATE media_files SET path=?, size=?, mtime=?, container=?, duration_s=?, bitrate=?, \
                 probe_json=?, edition=?, part_no=? WHERE id=?",
            )
            .bind(&job.entry.path_str)
            .bind(job.entry.size)
            .bind(job.entry.mtime)
            .bind(&pr.container)
            .bind(pr.duration_s)
            .bind(pr.bitrate)
            .bind(&pr.raw)
            .bind(&job.parsed.edition)
            .bind(job.parsed.part.unwrap_or(1))
            .bind(ex.id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM streams WHERE file_id = ?")
                .bind(ex.id)
                .execute(&mut *tx)
                .await?;
            (ex.id, false)
        }
        None => {
            let (item, kept_links) = match &forced {
                Some((item, links)) => (*item, Some(links.clone())),
                None => (
                    ensure_item(&mut tx, lib.id, &job.parsed, &mut new_created).await?,
                    None,
                ),
            };
            let r = sqlx::query(
                "INSERT INTO media_files (item_id, path, size, mtime, container, duration_s, \
                 bitrate, probe_json, edition, part_no) VALUES (?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(item)
            .bind(&job.entry.path_str)
            .bind(job.entry.size)
            .bind(job.entry.mtime)
            .bind(&pr.container)
            .bind(pr.duration_s)
            .bind(pr.bitrate)
            .bind(&pr.raw)
            .bind(&job.parsed.edition)
            .bind(job.parsed.part.unwrap_or(1))
            .execute(&mut *tx)
            .await?;
            let file_id = r.last_insert_rowid();
            match kept_links {
                Some(links) => {
                    for l in links {
                        sqlx::query("INSERT OR IGNORE INTO media_file_links (file_id, item_id) VALUES (?, ?)")
                            .bind(file_id)
                            .bind(l)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
                None => {
                    link_extra_episodes(
                        &mut tx,
                        lib.id,
                        file_id,
                        item,
                        &job.parsed,
                        &mut new_created,
                    )
                    .await?;
                }
            }
            (file_id, true)
        }
    };
    for s in &pr.streams {
        insert_stream(&mut tx, file_id, s).await?;
    }
    write_sidecars(&mut tx, file_id, &job.subs).await?;
    // Runtime hint for the item if unset.
    if let Some(d) = pr.duration_s {
        sqlx::query(
            "UPDATE media_items SET runtime_s = ? WHERE runtime_s IS NULL AND id = \
             (SELECT item_id FROM media_files WHERE id = ?)",
        )
        .bind(d.round() as i64)
        .bind(file_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    created.extend(new_created);
    Ok(is_new)
}

// ------------------------------------------------------------------ multi-episode links

/// Link a file to the further episodes it contains (`S01E02E03`), creating those episode items.
async fn link_extra_episodes(
    conn: &mut SqliteConnection,
    lib: i64,
    file_id: i64,
    primary_item: i64,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<()> {
    if p.kind != MediaKind::Episode || p.episodes.len() < 2 {
        return Ok(());
    }
    let Some(season): Option<i64> =
        sqlx::query_scalar("SELECT parent_id FROM media_items WHERE id = ?")
            .bind(primary_item)
            .fetch_optional(&mut *conn)
            .await?
            .flatten()
    else {
        return Ok(());
    };
    let season_no = p.season.unwrap_or(1);
    for &n in &p.episodes[1..] {
        let single = ParsedMedia {
            episodes: vec![n],
            episode_title: None,
            ..p.clone()
        };
        let item = ensure_episode(conn, lib, season, season_no, &single, created).await?;
        if item == primary_item {
            continue;
        }
        sqlx::query("INSERT OR IGNORE INTO media_file_links (file_id, item_id) VALUES (?, ?)")
            .bind(file_id)
            .bind(item)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Idempotent: make sure an unchanged multi-episode file has all its episode links (also repairs
/// libraries scanned before multi-episode support existed).
async fn ensure_links(
    pool: &SqlitePool,
    lib: &Library,
    file_id: i64,
    p: &ParsedMedia,
    created: &mut Vec<i64>,
) -> Result<()> {
    if p.kind != MediaKind::Episode || p.episodes.len() < 2 {
        return Ok(());
    }
    let have: i64 = sqlx::query_scalar("SELECT count(*) FROM media_file_links WHERE file_id = ?")
        .bind(file_id)
        .fetch_one(pool)
        .await?;
    if have as usize >= p.episodes.len() - 1 {
        return Ok(());
    }
    let primary: i64 = sqlx::query_scalar("SELECT item_id FROM media_files WHERE id = ?")
        .bind(file_id)
        .fetch_one(pool)
        .await?;
    let mut tx = pool.begin().await?;
    let mut new_created = Vec::new();
    link_extra_episodes(&mut tx, lib.id, file_id, primary, p, &mut new_created).await?;
    tx.commit().await?;
    created.extend(new_created);
    Ok(())
}

// ------------------------------------------------------------------ rename detection

enum Moved {
    /// The still-present row of the vanished file (re-pointed in place).
    Row(ExistingFile),
    /// The row was already deleted; re-attach the new file to its item.
    Tomb { item_id: i64, links: Vec<i64> },
}

type StreamKey = (
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn fingerprint(streams: impl Iterator<Item = StreamKey>) -> String {
    streams
        .map(|(k, c, w, h, ch)| {
            format!(
                "{k}:{}:{}x{}:{}",
                c.unwrap_or_default(),
                w.unwrap_or(0),
                h.unwrap_or(0),
                ch.unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn probe_fingerprint(pr: &ProbeResult) -> String {
    fingerprint(pr.streams.iter().map(|s| {
        (
            s.kind.as_str().to_string(),
            s.codec.clone(),
            s.width,
            s.height,
            s.channels,
        )
    }))
}

async fn db_fingerprint(pool: &SqlitePool, file_id: i64) -> Result<String> {
    let rows = sqlx::query(
        "SELECT kind, codec, width, height, channels FROM streams \
         WHERE file_id = ? AND external_path IS NULL ORDER BY idx",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    Ok(fingerprint(rows.iter().map(|r| {
        (
            r.get::<String, _>("kind"),
            r.get("codec"),
            r.get("width"),
            r.get("height"),
            r.get("channels"),
        )
    })))
}

fn same_duration(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1.0,
        (None, None) => true,
        _ => false,
    }
}

fn stem_of(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

/// Tie-break between identical candidates: how much of the file name is shared.
fn name_affinity(a: &str, b: &str) -> usize {
    let pre = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let suf = a
        .chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .count();
    pre + suf
}

async fn tomb_of(
    pool: &SqlitePool,
    library_id: i64,
    ex: &ExistingFile,
    path: &str,
) -> Result<Option<Tomb>> {
    let duration: Option<f64> =
        sqlx::query_scalar("SELECT duration_s FROM media_files WHERE id = ?")
            .bind(ex.id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let links: Vec<i64> =
        sqlx::query_scalar("SELECT item_id FROM media_file_links WHERE file_id = ?")
            .bind(ex.id)
            .fetch_all(pool)
            .await?;
    Ok(Some(Tomb {
        library_id,
        item_id: ex.item_id,
        links,
        size: ex.size,
        duration,
        fingerprint: db_fingerprint(pool, ex.id).await?,
        stem: stem_of(path),
        at: Instant::now(),
    }))
}

impl Scanner {
    /// Is the newly found file a moved/renamed copy of a file that vanished? Same size, same
    /// duration (±1 s) and same stream layout; among several candidates the most similar name wins.
    async fn find_moved(
        &self,
        pool: &SqlitePool,
        library_id: i64,
        job: &Job,
        pr: &ProbeResult,
        vanished: &HashMap<i64, Vec<(String, ExistingFile)>>,
        claimed: &HashSet<i64>,
    ) -> Result<Option<Moved>> {
        let fp = probe_fingerprint(pr);
        let new_stem = stem_of(&job.entry.path_str);
        let mut best: Option<(usize, ExistingFile)> = None;
        for (path, ex) in vanished.get(&job.entry.size).into_iter().flatten() {
            if claimed.contains(&ex.id) || tokio::fs::try_exists(path).await.unwrap_or(true) {
                continue;
            }
            let dur: Option<f64> =
                sqlx::query_scalar("SELECT duration_s FROM media_files WHERE id = ?")
                    .bind(ex.id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
            if !same_duration(dur, pr.duration_s) || db_fingerprint(pool, ex.id).await? != fp {
                continue;
            }
            let score = name_affinity(&stem_of(path), &new_stem);
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, ex.clone()));
            }
        }
        if let Some((_, ex)) = best {
            return Ok(Some(Moved::Row(ex)));
        }
        let mut tombs = self.tombs.lock().unwrap();
        let window = self.rename_window;
        tombs.retain(|t| t.at.elapsed() <= window);
        let pick = tombs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.library_id == library_id
                    && t.size == job.entry.size
                    && same_duration(t.duration, pr.duration_s)
                    && t.fingerprint == fp
            })
            .max_by_key(|(_, t)| name_affinity(&t.stem, &new_stem))
            .map(|(i, _)| i);
        Ok(pick.map(|i| {
            let t = tombs.remove(i);
            Moved::Tomb {
                item_id: t.item_id,
                links: t.links,
            }
        }))
    }
}

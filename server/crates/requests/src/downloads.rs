//! Aggregated view of *all* Radarr/Sonarr queue activity (not only requests made through Reel),
//! cached for a couple of seconds so many polling clients never hammer the Arrs.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use reel_core::RequestState;
use serde::Serialize;

use crate::{
    ArrKind, MediaType, Result,
    arr::{ArrClient, QueueItem, QueueMedia, Radarr, Sonarr, parse_timeleft},
    models::{ARR_COLS, ArrServer, ArrServerRow, REQUEST_COLS, RequestRecord, RequestRow},
    service::{RequestsService, library_item_ids},
    shared::SharedRef,
    state::{classify, item_progress},
    torrent::{TorrentClient, TorrentClientConfig, TorrentInfo, TorrentKind},
};

/// How long a snapshot is reused.
pub const SNAPSHOT_TTL: Duration = Duration::from_secs(2);

/// One Arr queue record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DownloadItem {
    /// `"{arr_server_id}:{queue_id}"`
    pub id: String,
    pub arr_server_id: i64,
    pub arr_name: String,
    pub media_type: MediaType,
    pub title: String,
    pub release_title: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub episode_title: Option<String>,
    pub state: RequestState,
    /// 0..1
    pub progress: f32,
    pub eta_seconds: Option<u64>,
    pub size_bytes: Option<u64>,
    pub sizeleft_bytes: Option<u64>,
    pub speed_bps: Option<u64>,
    pub error: Option<String>,
    /// Reel request this download belongs to, if any.
    pub request_id: Option<i64>,
    /// Owner of that request (for per-user filtering; not serialized).
    #[serde(skip)]
    pub request_user_id: Option<i64>,
    pub library_item_id: Option<i64>,
    /// TMDB poster path (`/abc.jpg`).
    pub poster_path: Option<String>,
}

#[derive(Default)]
pub(crate) struct DlCache {
    at: Option<Instant>,
    items: Arc<Vec<DownloadItem>>,
    /// queue item id → (last sizeleft, when it last changed), for stall detection.
    mem: HashMap<String, (f64, Instant)>,
    torrent: Option<(TorrentClientConfig, Arc<TorrentClient>)>,
}

fn request_matches(req: &RequestRecord, sid: i64, item: &QueueItem, kind: ArrKind) -> bool {
    let (Some(arr), Some(id)) = (req.arr_id, req.arr_item_id) else {
        return false;
    };
    if arr != sid {
        return false;
    }
    match kind {
        ArrKind::Radarr => req.media_type == MediaType::Movie && item.movie_id == Some(id),
        ArrKind::Sonarr => {
            req.media_type == MediaType::Tv
                && item.series_id == Some(id)
                && match (&req.seasons, item.season()) {
                    (Some(sel), Some(n)) => sel.contains(&n),
                    _ => true,
                }
        }
    }
}

/// `/t/p/original/abc.jpg` → `/abc.jpg` for TMDB-hosted artwork.
fn tmdb_path_of(url: &str) -> Option<String> {
    let rest = url.split("image.tmdb.org/t/p/").nth(1)?;
    let (_size, path) = rest.split_once('/')?;
    Some(format!("/{path}"))
}

fn poster_of(m: Option<&QueueMedia>) -> Option<String> {
    m?.images
        .iter()
        .find(|i| i.cover_type.as_deref() == Some("poster"))
        .and_then(|i| i.remote_url.as_deref())
        .and_then(tmdb_path_of)
}

async fn fetch_queue(server: &ArrServer, page: u32) -> Result<Vec<QueueItem>> {
    let c = ArrClient::new(&server.url, &server.api_key).with_page_size(page);
    match server.kind {
        ArrKind::Radarr => Radarr(c).queue().await,
        ArrKind::Sonarr => Sonarr(c).queue().await,
    }
}

impl RequestsService {
    /// Every item in every Radarr/Sonarr queue, newest data at most [`SNAPSHOT_TTL`] old.
    /// Concurrent callers share one refresh. Unreachable servers are skipped.
    pub async fn downloads(&self) -> Result<Arc<Vec<DownloadItem>>> {
        let mut cache = self.s.downloads.lock().await;
        if let Some(at) = cache.at {
            if at.elapsed() < SNAPSHOT_TTL {
                return Ok(cache.items.clone());
            }
        }
        let items = Arc::new(build_snapshot(&self.s, &mut cache).await?);
        cache.items = items.clone();
        cache.at = Some(Instant::now());
        Ok(items)
    }
}

async fn torrent_client(s: &SharedRef, cache: &mut DlCache) {
    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT kind, url, username, password FROM download_clients ORDER BY id LIMIT 1",
    )
    .fetch_optional(&s.pool)
    .await
    .ok()
    .flatten();
    let cfg = row.and_then(|(kind, url, username, password)| {
        let kind = match kind.as_str() {
            "qbittorrent" => TorrentKind::Qbittorrent,
            "transmission" => TorrentKind::Transmission,
            _ => return None,
        };
        Some(TorrentClientConfig {
            kind,
            url,
            username,
            password,
        })
    });
    match cfg {
        None => cache.torrent = None,
        Some(cfg) => {
            if cache.torrent.as_ref().map(|(c, _)| c) != Some(&cfg) {
                let c = Arc::new(TorrentClient::from_config(&cfg));
                cache.torrent = Some((cfg, c));
            }
        }
    }
}

async fn build_snapshot(s: &SharedRef, cache: &mut DlCache) -> Result<Vec<DownloadItem>> {
    let servers: Vec<ArrServer> = sqlx::query_as::<_, ArrServerRow>(&format!(
        "SELECT {ARR_COLS} FROM arr_servers ORDER BY id"
    ))
    .fetch_all(&s.pool)
    .await?
    .into_iter()
    .map(Into::into)
    .collect();
    if servers.is_empty() {
        cache.mem.clear();
        return Ok(vec![]);
    }

    let mut set = tokio::task::JoinSet::new();
    for srv in servers {
        let page = s.cfg.queue_page_size;
        set.spawn(async move {
            let q = fetch_queue(&srv, page).await;
            (srv, q)
        });
    }
    let mut queues: Vec<(ArrServer, Vec<QueueItem>)> = vec![];
    while let Some(r) = set.join_next().await {
        let Ok((srv, q)) = r else { continue };
        match q {
            Ok(q) => queues.push((srv, q)),
            Err(e) => tracing::warn!(server = %srv.name, error = %e, "arr queue poll failed"),
        }
    }
    queues.sort_by_key(|(s, _)| s.id);
    if queues.iter().all(|(_, q)| q.is_empty()) {
        cache.mem.clear();
        return Ok(vec![]);
    }

    let requests: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
        "SELECT {REQUEST_COLS} FROM requests WHERE arr_id IS NOT NULL AND arr_item_id IS NOT NULL
         AND status != 'declined' ORDER BY id DESC LIMIT 5000"
    ))
    .fetch_all(&s.pool)
    .await?
    .into_iter()
    .map(Into::into)
    .collect();

    // Optional torrent-client enrichment (speed, precise progress).
    torrent_client(s, cache).await;
    let hashes: Vec<String> = queues
        .iter()
        .flat_map(|(_, q)| q.iter())
        .filter_map(|i| i.download_id.as_deref())
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let torrents: HashMap<String, TorrentInfo> = match (&cache.torrent, hashes.is_empty()) {
        (Some((_, tc)), false) => tc.info(&hashes).await.unwrap_or_default(),
        _ => HashMap::new(),
    };

    let now = Instant::now();
    let mut seen_ids = HashSet::new();
    let mut out: Vec<DownloadItem> = vec![];
    for (srv, queue) in &queues {
        for q in queue {
            let id = format!("{}:{}", srv.id, q.id);
            seen_ids.insert(id.clone());
            let req = requests
                .iter()
                .find(|r| request_matches(r, srv.id, q, srv.kind));
            let media = match srv.kind {
                ArrKind::Radarr => q.movie.as_ref(),
                ArrKind::Sonarr => q.series.as_ref(),
            };
            let mut state = classify(q);
            let mut progress = item_progress(q);
            let size = q.size.max(0.0);
            if size <= 0.0 && state == RequestState::Importing {
                progress = 1.0;
            }
            let mut eta = matches!(state, RequestState::Downloading | RequestState::Queued)
                .then(|| q.timeleft.as_deref().and_then(parse_timeleft))
                .flatten();
            let mut speed = None;
            if let Some(t) = q
                .download_id
                .as_deref()
                .and_then(|d| torrents.get(&d.to_ascii_lowercase()))
            {
                speed = Some(t.speed_bps);
                if matches!(state, RequestState::Downloading | RequestState::Queued) {
                    progress = t.progress.clamp(0.0, 1.0);
                    eta = t.eta_seconds.or(eta);
                }
            }
            // stall detection: no bytes for `stall_after`
            let m = cache.mem.entry(id.clone()).or_insert((q.sizeleft, now));
            if (m.0 - q.sizeleft).abs() > f64::EPSILON {
                *m = (q.sizeleft, now);
            }
            if state == RequestState::Downloading && now.duration_since(m.1) >= s.cfg.stall_after {
                state = RequestState::Stalled;
            }
            let tmdb_id = media
                .and_then(|m| m.tmdb_id)
                .filter(|i| *i > 0)
                .or_else(|| req.map(|r| r.tmdb_id));
            out.push(DownloadItem {
                id,
                arr_server_id: srv.id,
                arr_name: srv.name.clone(),
                media_type: match srv.kind {
                    ArrKind::Radarr => MediaType::Movie,
                    ArrKind::Sonarr => MediaType::Tv,
                },
                title: media
                    .and_then(|m| m.title.clone())
                    .or_else(|| req.and_then(|r| r.title.clone()))
                    .or_else(|| q.title.clone())
                    .unwrap_or_else(|| "Unknown".into()),
                release_title: q.title.clone(),
                tmdb_id,
                tvdb_id: media.and_then(|m| m.tvdb_id).filter(|i| *i > 0),
                season: q.season(),
                episode: q.episode_number(),
                episode_title: q.episode.as_ref().and_then(|e| e.title.clone()),
                state,
                progress,
                eta_seconds: eta,
                size_bytes: (size > 0.0).then_some(size as u64),
                sizeleft_bytes: (size > 0.0).then(|| q.sizeleft.clamp(0.0, size) as u64),
                speed_bps: speed,
                error: q.error_message.clone().filter(|e| !e.is_empty()),
                request_id: req.map(|r| r.id),
                request_user_id: req.map(|r| r.user_id),
                library_item_id: None,
                poster_path: req
                    .and_then(|r| r.poster.clone())
                    .or_else(|| poster_of(media)),
            });
        }
    }
    cache.mem.retain(|k, _| seen_ids.contains(k));

    // Sonarr series without a TMDB id: map through the library by tvdb id.
    let tvdbs: Vec<i64> = out
        .iter()
        .filter(|i| i.tmdb_id.is_none())
        .filter_map(|i| i.tvdb_id)
        .collect();
    if !tvdbs.is_empty() {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT tvdb_id, tmdb_id FROM media_items WHERE kind = 'show' AND tmdb_id IS NOT NULL
             AND tvdb_id IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(&tvdbs).unwrap_or_default())
        .fetch_all(&s.pool)
        .await?;
        let map: HashMap<i64, i64> = rows.into_iter().collect();
        for i in out.iter_mut().filter(|i| i.tmdb_id.is_none()) {
            i.tmdb_id = i.tvdb_id.and_then(|t| map.get(&t).copied());
        }
    }

    // Library links.
    for mt in [MediaType::Movie, MediaType::Tv] {
        let ids: Vec<i64> = out
            .iter()
            .filter(|i| i.media_type == mt)
            .filter_map(|i| i.tmdb_id)
            .collect();
        let map: HashMap<i64, i64> = library_item_ids(&s.pool, mt, &ids)
            .await?
            .into_iter()
            .collect();
        for i in out.iter_mut().filter(|i| i.media_type == mt) {
            i.library_item_id = i.tmdb_id.and_then(|t| map.get(&t).copied());
        }
    }

    // TMDB poster for titles that have neither (cached by the client for an hour).
    if let Some(tmdb) = &s.tmdb {
        let mut looked: HashMap<(MediaType, i64), Option<String>> = HashMap::new();
        for i in out.iter_mut().filter(|i| i.poster_path.is_none()) {
            let Some(t) = i.tmdb_id else { continue };
            if looked.len() >= 20 && !looked.contains_key(&(i.media_type, t)) {
                continue;
            }
            let p = match looked.get(&(i.media_type, t)) {
                Some(p) => p.clone(),
                None => {
                    let p = match i.media_type {
                        MediaType::Movie => {
                            tmdb.movie_cached(t).await.ok().and_then(|m| m.poster_path)
                        }
                        MediaType::Tv => tmdb.tv_cached(t).await.ok().and_then(|m| m.poster_path),
                    };
                    looked.insert((i.media_type, t), p.clone());
                    p
                }
            };
            i.poster_path = p;
        }
    }
    Ok(out)
}

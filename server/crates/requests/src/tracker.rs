//! Download tracker: polls Arr queues while requests are in flight and drives the
//! SPEC §7.1 state machine.

use std::{collections::HashMap, time::Instant};

use reel_core::RequestState;
use tokio::time::sleep;

use crate::{
    ArrKind, MediaType, Result,
    arr::{ArrClient, QueueItem, Radarr, Sonarr},
    models::{ArrServer, Progress, REQUEST_COLS, RequestRecord, RequestRow, TRACKED_STATES},
    shared::SharedRef,
    state::{aggregate, apply_stall},
    torrent::{TorrentClient, TorrentClientConfig, TorrentKind},
};

struct Mem {
    last_sizeleft: Option<u64>,
    last_change: Instant,
    state_since: Instant,
}

impl Mem {
    fn new() -> Self {
        let now = Instant::now();
        Mem {
            last_sizeleft: None,
            last_change: now,
            state_since: now,
        }
    }
}

pub(crate) struct Tracker {
    s: SharedRef,
    mem: HashMap<i64, Mem>,
    torrent: Option<(TorrentClientConfig, TorrentClient)>,
}

fn matches(req: &RequestRecord, item: &QueueItem) -> bool {
    let Some(id) = req.arr_item_id else {
        return false;
    };
    match req.media_type {
        MediaType::Movie => item.movie_id == Some(id),
        MediaType::Tv => {
            item.series_id == Some(id)
                && match (&req.seasons, item.season()) {
                    (Some(sel), Some(n)) => sel.contains(&n),
                    _ => true,
                }
        }
    }
}

fn client(server: &ArrServer, page: u32) -> ArrClient {
    ArrClient::new(&server.url, &server.api_key).with_page_size(page)
}

impl Tracker {
    pub fn new(s: SharedRef) -> Self {
        Tracker {
            s,
            mem: HashMap::new(),
            torrent: None,
        }
    }

    /// Run forever: poll while active, otherwise sleep until woken.
    pub async fn run(mut self) {
        loop {
            let active = match self.tick().await {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(error = %e, "download tracker tick failed");
                    true
                }
            };
            let wait = if active {
                self.s.cfg.poll_interval
            } else {
                self.s.cfg.idle_recheck
            };
            tokio::select! {
                _ = self.s.wake.notified() => {}
                _ = sleep(wait) => {}
            }
        }
    }

    async fn refresh_torrent(&mut self) {
        let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT kind, url, username, password FROM download_clients ORDER BY id LIMIT 1",
        )
        .fetch_optional(&self.s.pool)
        .await
        .ok()
        .flatten();
        let Some((kind, url, username, password)) = row else {
            self.torrent = None;
            return;
        };
        let kind = match kind.as_str() {
            "qbittorrent" => TorrentKind::Qbittorrent,
            "transmission" => TorrentKind::Transmission,
            _ => {
                self.torrent = None;
                return;
            }
        };
        let cfg = TorrentClientConfig {
            kind,
            url,
            username,
            password,
        };
        if self.torrent.as_ref().map(|(c, _)| c) != Some(&cfg) {
            let c = TorrentClient::from_config(&cfg);
            self.torrent = Some((cfg, c));
        }
    }

    /// One polling pass. Returns whether any request is still in flight.
    pub async fn tick(&mut self) -> Result<bool> {
        let active: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests WHERE status IN ({TRACKED_STATES}) ORDER BY id"
        ))
        .fetch_all(&self.s.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
        if active.is_empty() {
            self.mem.clear();
            return Ok(false);
        }
        self.mem.retain(|id, _| active.iter().any(|r| r.id == *id));
        self.refresh_torrent().await;

        let mut by_server: HashMap<i64, Vec<RequestRecord>> = HashMap::new();
        for r in active {
            if let Some(sid) = r.arr_id {
                by_server.entry(sid).or_default().push(r);
            }
        }
        for (sid, reqs) in by_server {
            let Some(server) = self.s.get_server(sid).await? else {
                continue;
            };
            let c = client(&server, self.s.cfg.queue_page_size);
            let queue = match server.kind {
                ArrKind::Radarr => Radarr(c.clone()).queue().await,
                ArrKind::Sonarr => Sonarr(c.clone()).queue().await,
            };
            let queue = match queue {
                Ok(q) => q,
                Err(e) => {
                    tracing::warn!(server = %server.name, error = %e, "arr queue poll failed");
                    continue;
                }
            };
            for req in reqs {
                if let Err(e) = self.process(&req, &server, &c, &queue).await {
                    tracing::warn!(request = req.id, error = %e, "tracking request failed");
                }
            }
        }
        Ok(true)
    }

    async fn process(
        &mut self,
        req: &RequestRecord,
        server: &ArrServer,
        c: &ArrClient,
        queue: &[QueueItem],
    ) -> Result<()> {
        let mine: Vec<&QueueItem> = queue.iter().filter(|i| matches(req, i)).collect();
        let now = Instant::now();
        let Some(agg) = aggregate(&mine) else {
            return self.no_items(req, server, c).await;
        };

        let mut p = Progress::basic(req, agg.state);
        p.progress = agg.progress;
        p.eta_seconds = agg.eta_seconds;
        p.size_bytes = Some(agg.size);
        p.release_title = agg.release_title.clone();
        p.error = agg.error.clone();
        p.items = agg.items.clone();

        // Optional torrent-client enrichment.
        if let Some((_, tc)) = &self.torrent {
            let hashes: Vec<String> = mine
                .iter()
                .filter_map(|i| i.download_id.as_deref())
                .map(str::to_ascii_lowercase)
                .collect();
            let info = tc.info(&hashes).await.unwrap_or_default();
            if !info.is_empty() {
                p.speed_bps = Some(info.values().map(|t| t.speed_bps).sum());
                p.seeds = Some(info.values().map(|t| t.seeds).sum());
                p.peers = Some(info.values().map(|t| t.peers).sum());
                if matches!(agg.state, RequestState::Downloading | RequestState::Queued) {
                    // More precise size-weighted progress straight from the client.
                    let (mut size, mut done) = (0.0f64, 0.0f64);
                    for i in &mine {
                        let sz = i.size.max(0.0);
                        let pr = i
                            .download_id
                            .as_deref()
                            .and_then(|d| info.get(&d.to_ascii_lowercase()))
                            .map(|t| t.progress as f64)
                            .unwrap_or_else(|| crate::state::item_progress(i) as f64);
                        size += sz;
                        done += sz * pr;
                    }
                    if size > 0.0 {
                        p.progress = (done / size).clamp(0.0, 1.0) as f32;
                    }
                    if let Some(eta) = info.values().filter_map(|t| t.eta_seconds).max() {
                        p.eta_seconds = Some(eta);
                    }
                }
            }
        }

        let mem = self.mem.entry(req.id).or_insert_with(Mem::new);
        if mem.last_sizeleft != Some(agg.sizeleft) {
            mem.last_sizeleft = Some(agg.sizeleft);
            mem.last_change = now;
        }
        p.state = apply_stall(
            agg.state,
            now.duration_since(mem.last_change),
            self.s.cfg.stall_after,
        );

        let importing = p.state == RequestState::Importing;
        let changed = self.s.transition(req, p.clone()).await?;
        if changed {
            mem.state_since = now;
            self.s.persist_downloads(req.id, &p).await?;
        }
        if importing && self.s.library_has(req.media_type, req.tmdb_id).await? {
            self.s.mark_available(req).await?;
        }
        Ok(())
    }

    /// No queue entries for this request.
    async fn no_items(
        &mut self,
        req: &RequestRecord,
        server: &ArrServer,
        c: &ArrClient,
    ) -> Result<()> {
        let since = self.mem.entry(req.id).or_insert_with(Mem::new).state_since;
        match req.status {
            RequestState::Searching => {
                let (Some(item), true) =
                    (req.arr_item_id, since.elapsed() >= self.s.cfg.search_grace)
                else {
                    return Ok(());
                };
                let cmds = c.commands().await?;
                let latest = cmds.iter().filter(|x| x.targets(item)).max_by_key(|x| x.id);
                let done = match latest {
                    Some(cmd) => cmd.is_finished(),
                    None => since.elapsed() >= self.s.cfg.search_grace * 3,
                };
                if done {
                    let p = Progress::basic(req, RequestState::NotFound);
                    self.s.transition(req, p).await?;
                }
            }
            RequestState::Queued | RequestState::Downloading | RequestState::Stalled => {
                // Left the queue: imported already, or removed/blocklisted (Arr re-searches).
                let state = if self.has_file(req, server).await {
                    RequestState::Importing
                } else {
                    RequestState::Searching
                };
                let mut p = Progress::basic(req, state);
                if state == RequestState::Importing {
                    p.progress = 1.0;
                }
                let changed = self.s.transition(req, p).await?;
                if changed {
                    if let Some(m) = self.mem.get_mut(&req.id) {
                        m.state_since = Instant::now();
                    }
                }
            }
            RequestState::Importing => {
                if self.s.library_has(req.media_type, req.tmdb_id).await? {
                    self.s.mark_available(req).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn has_file(&self, req: &RequestRecord, server: &ArrServer) -> bool {
        let Some(id) = req.arr_item_id else {
            return false;
        };
        match req.media_type {
            MediaType::Movie => Radarr::new(&server.url, &server.api_key)
                .movie(id)
                .await
                .map(|m| m.has_file)
                .unwrap_or(false),
            MediaType::Tv => {
                let Ok(eps) = Sonarr::new(&server.url, &server.api_key).episodes(id).await else {
                    return false;
                };
                let scope: Vec<_> = eps
                    .iter()
                    .filter(|e| e.monitored)
                    .filter(|e| match &req.seasons {
                        Some(sel) => sel.contains(&e.season_number),
                        None => e.season_number > 0,
                    })
                    .collect();
                scope.iter().any(|e| e.has_file)
            }
        }
    }
}

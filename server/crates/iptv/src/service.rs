//! The IPTV service: owns the hub, live sessions, ingest and the refresh scheduler.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::{
    ChannelHub, Error, HubConfig, LiveManager, Result,
    ingest::{Ingest, RefreshReport},
    store::{self, DbResolver, Source},
};

#[derive(Debug, Clone)]
pub struct IptvConfig {
    pub hub: HubConfig,
    /// Days of EPG kept in the past / future.
    pub epg_days_back: u32,
    pub epg_days_forward: u32,
    /// Refresh interval for sources without one (minutes).
    pub default_refresh_minutes: i64,
    /// Keep this many most-favorited channels warm (0 = off).
    pub warm_favorites: usize,
    /// Drop a live session nobody streams from after this long.
    pub session_idle: Duration,
    /// Allow playlists, guides, provider APIs and stream upstreams on private networks.
    pub allow_private_fetch: bool,
    /// Allow channel logos to be fetched from private networks (logos come from third-party
    /// playlists, so this defaults to off).
    pub allow_private_logo_fetch: bool,
}

impl Default for IptvConfig {
    fn default() -> Self {
        Self {
            hub: HubConfig::default(),
            epg_days_back: 1,
            epg_days_forward: 7,
            default_refresh_minutes: 12 * 60,
            warm_favorites: 0,
            session_idle: Duration::from_secs(30),
            allow_private_fetch: true,
            allow_private_logo_fetch: false,
        }
    }
}

pub struct IptvService {
    pub pool: SqlitePool,
    pub ingest: Ingest,
    pub live: Arc<LiveManager>,
    pub cfg: IptvConfig,
    /// HTTP client for channel logos (guarded unless `allow_private_logo_fetch`).
    pub logo_http: reqwest::Client,
    pub data_dir: PathBuf,
    refreshing: Mutex<HashSet<i64>>,
}

impl IptvService {
    pub fn new(pool: SqlitePool, data_dir: PathBuf, cfg: IptvConfig) -> Arc<Self> {
        let mut hub_cfg = cfg.hub.clone();
        hub_cfg.allow_private_upstream = cfg.allow_private_fetch;
        let hub = ChannelHub::new(hub_cfg, Arc::new(DbResolver { pool: pool.clone() }));
        let live = LiveManager::new(hub, data_dir.join("iptv").join("hls"), cfg.session_idle);
        let http = crate::net::client(cfg.allow_private_fetch);
        let logo_http = crate::net::client(cfg.allow_private_logo_fetch);
        Arc::new(Self {
            ingest: Ingest {
                pool: pool.clone(),
                http,
                work_dir: data_dir.join("iptv").join("tmp"),
                epg_back_secs: i64::from(cfg.epg_days_back) * 86_400,
                epg_fwd_secs: i64::from(cfg.epg_days_forward) * 86_400,
                allow_private: cfg.allow_private_fetch,
            },
            logo_http,
            pool,
            live,
            cfg,
            data_dir,
            refreshing: Mutex::default(),
        })
    }

    pub fn hub(&self) -> &ChannelHub {
        self.live.hub()
    }

    /// Reapers, the refresh scheduler and the warm-favorites task.
    pub fn spawn_background(self: &Arc<Self>) {
        self.hub().spawn_reaper();
        self.live.spawn_reaper();
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                me.refresh_due().await;
            }
        });
        if self.cfg.warm_favorites > 0 {
            let me = self.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tick.tick().await;
                    me.warm_favorites().await;
                }
            });
        }
    }

    async fn warm_favorites(&self) {
        let Ok(ids) = store::top_favorites(&self.pool, self.cfg.warm_favorites).await else {
            return;
        };
        self.hub().set_pinned(&ids.iter().copied().collect());
        for id in ids {
            if let Err(e) = self.hub().warm(id, true).await {
                warn!(channel = id, "cannot warm favorite: {e}");
            }
        }
    }

    async fn refresh_due(self: &Arc<Self>) {
        let Ok(sources) = store::list_sources(&self.pool).await else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for s in sources.into_iter().filter(|s| s.enabled) {
            let minutes = s
                .refresh_interval
                .unwrap_or(self.cfg.default_refresh_minutes);
            let due = match s.last_refresh_at {
                None => true,
                Some(t) => minutes > 0 && now - t >= minutes * 60,
            };
            if due {
                self.spawn_refresh(s.id);
            }
        }
    }

    /// Refresh in the background (no-op if one is already running for the source).
    pub fn spawn_refresh(self: &Arc<Self>, source_id: i64) {
        let me = self.clone();
        tokio::spawn(async move {
            match me.refresh_source(source_id).await {
                Ok(r) => info!(source_id, channels = r.channels.total, "source refreshed"),
                Err(Error::Invalid(m)) if m.contains("already") => {}
                Err(e) => warn!(source_id, "source refresh failed: {e}"),
            }
        });
    }

    pub fn is_refreshing(&self, source_id: i64) -> bool {
        self.refreshing.lock().unwrap().contains(&source_id)
    }

    /// Refresh one source now (channels + EPG).
    pub async fn refresh_source(&self, source_id: i64) -> Result<RefreshReport> {
        if !self.refreshing.lock().unwrap().insert(source_id) {
            return Err(Error::Invalid(
                "a refresh is already running for this source".into(),
            ));
        }
        struct Done<'a>(&'a IptvService, i64);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.refreshing.lock().unwrap().remove(&self.1);
            }
        }
        let _done = Done(self, source_id);
        let src: Source = store::get_source(&self.pool, source_id).await?;
        self.ingest.refresh_source(&src).await
    }

    /// Re-run only the guide import of one source (in the background; same single-flight guard).
    pub fn spawn_epg_refresh(self: &Arc<Self>, source_id: i64) {
        if !self.refreshing.lock().unwrap().insert(source_id) {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let res = async {
                let src = store::get_source(&me.pool, source_id).await?;
                me.ingest.refresh_epg_recorded(&src).await
            }
            .await;
            me.refreshing.lock().unwrap().remove(&source_id);
            match res {
                Ok(r) => info!(
                    source_id,
                    programmes = r.programmes_inserted,
                    "guide refreshed"
                ),
                Err(e) => warn!(
                    source_id,
                    "guide refresh failed: {}",
                    crate::net::redact_text(&e.to_string())
                ),
            }
        });
    }

    pub async fn shutdown(&self) {
        self.live.shutdown().await;
    }
}

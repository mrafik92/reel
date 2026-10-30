//! Live viewing sessions: tune → session → passthrough TS / shared HLS.

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::Stream;
use serde::{Deserialize, Serialize};

use crate::{
    ChannelHub, Error, Result, Subscription,
    hls::{AudioMode, HlsLease, HlsManager, HlsOptions, VideoMode, valid_hls_file},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveMode {
    /// Raw MPEG-TS for native players (zero CPU).
    Passthrough,
    /// fMP4 HLS for browsers.
    Hls,
}

#[derive(Debug, Clone, Serialize)]
pub struct HlsDetail {
    pub video: VideoMode,
    pub audio: AudioMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveSessionInfo {
    pub id: String,
    pub user_id: i64,
    pub channel_id: i64,
    pub channel_name: String,
    pub mode: LiveMode,
    /// Unix seconds.
    pub started_at: i64,
    pub idle_s: u64,
    pub connections: usize,
    pub hls: Option<HlsDetail>,
}

struct Session {
    id: String,
    user_id: i64,
    channel_id: i64,
    name: String,
    mode: LiveMode,
    started_at: i64,
    last_seen: Mutex<Instant>,
    ts_active: AtomicUsize,
    /// Subscription taken at tune time (reserves the provider slot) until the player connects.
    pre: Mutex<Option<Subscription>>,
    lease: Option<HlsLease>,
}

pub struct LiveManager {
    hub: ChannelHub,
    hls: HlsManager,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    idle: Duration,
}

struct ActiveGuard(Arc<Session>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.ts_active.fetch_sub(1, Relaxed);
        *self.0.last_seen.lock().unwrap() = Instant::now();
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl LiveManager {
    /// `session_idle`: a session nobody streams from for this long is dropped.
    pub fn new(hub: ChannelHub, hls_root: PathBuf, session_idle: Duration) -> Arc<Self> {
        let hls = HlsManager::new(
            hls_root,
            hub.config().ffmpeg_path.clone(),
            Duration::from_secs(10),
        );
        Arc::new(Self {
            hub,
            hls,
            sessions: Mutex::default(),
            idle: session_idle,
        })
    }

    pub fn hub(&self) -> &ChannelHub {
        &self.hub
    }

    pub fn spawn_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let m = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                m.reap().await;
            }
        })
    }

    pub async fn reap(&self) {
        {
            let mut s = self.sessions.lock().unwrap();
            let idle = self.idle;
            s.retain(|_, v| {
                v.ts_active.load(Relaxed) > 0 || v.last_seen.lock().unwrap().elapsed() < idle
            });
        }
        self.hls.reap().await;
    }

    /// Start watching `channel_id`. Fails fast (before any player connects) with
    /// [`Error::BudgetExhausted`] when the provider limit is fully in use.
    pub async fn tune(
        &self,
        user_id: i64,
        channel_id: i64,
        mode: LiveMode,
        opts: &HlsOptions,
    ) -> Result<LiveSessionInfo> {
        let (pre, lease, name) = match mode {
            LiveMode::Passthrough => {
                let sub = self.hub.subscribe(channel_id, Some(user_id)).await?;
                let name = sub.pipe_metrics().name;
                (Some(sub), None, name)
            }
            LiveMode::Hls => {
                let lease = self
                    .hls
                    .acquire(&self.hub, channel_id, Some(user_id), opts)
                    .await?;
                let name = self
                    .hub
                    .metrics()
                    .into_iter()
                    .find(|m| m.channel_id == channel_id)
                    .map(|m| m.name)
                    .unwrap_or_default();
                (None, Some(lease), name)
            }
        };
        let s = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            user_id,
            channel_id,
            name,
            mode,
            started_at: now_unix(),
            last_seen: Mutex::new(Instant::now()),
            ts_active: AtomicUsize::new(0),
            pre: Mutex::new(pre),
            lease,
        });
        let info = self.info_of(&s);
        self.sessions.lock().unwrap().insert(s.id.clone(), s);
        Ok(info)
    }

    fn info_of(&self, s: &Session) -> LiveSessionInfo {
        LiveSessionInfo {
            id: s.id.clone(),
            user_id: s.user_id,
            channel_id: s.channel_id,
            channel_name: s.name.clone(),
            mode: s.mode,
            started_at: s.started_at,
            idle_s: s.last_seen.lock().unwrap().elapsed().as_secs(),
            connections: s.ts_active.load(Relaxed),
            hls: s.lease.as_ref().map(|l| HlsDetail {
                video: l.pipe().video,
                audio: l.pipe().audio,
            }),
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound("live session".into()))
    }

    pub fn session(&self, id: &str) -> Option<LiveSessionInfo> {
        self.get(id).ok().map(|s| self.info_of(&s))
    }

    pub fn list(&self) -> Vec<LiveSessionInfo> {
        let mut v: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| self.info_of(s))
            .collect();
        v.sort_by_key(|s| s.started_at);
        v
    }

    pub fn count_for_user(&self, user_id: i64) -> usize {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.user_id == user_id)
            .count()
    }

    pub fn stop(&self, id: &str) -> bool {
        self.sessions.lock().unwrap().remove(id).is_some()
    }

    /// Raw MPEG-TS byte stream of a passthrough session. Dropping it frees the viewer.
    pub async fn ts_stream(
        &self,
        id: &str,
    ) -> Result<impl Stream<Item = io::Result<Bytes>> + Send + use<>> {
        let s = self.get(id)?;
        if s.mode != LiveMode::Passthrough {
            return Err(Error::Invalid("this session is HLS; use index.m3u8".into()));
        }
        let pre = s.pre.lock().unwrap().take();
        let sub = match pre {
            Some(p) => p,
            None => self.hub.subscribe(s.channel_id, Some(s.user_id)).await?,
        };
        s.ts_active.fetch_add(1, Relaxed);
        let guard = ActiveGuard(s);
        let st = futures_util::stream::unfold((sub, guard), |(mut sub, g)| async move {
            sub.next().await.map(|b| (Ok(b), (sub, g)))
        });
        Ok(st)
    }

    /// Path of an HLS file of the session (waiting for the playlist to appear first).
    pub async fn hls_file(&self, id: &str, name: &str) -> Result<PathBuf> {
        let s = self.get(id)?;
        *s.last_seen.lock().unwrap() = Instant::now();
        let lease = s
            .lease
            .as_ref()
            .ok_or_else(|| Error::Invalid("this session is passthrough; use stream.ts".into()))?;
        if !valid_hls_file(name) {
            return Err(Error::NotFound("file".into()));
        }
        let pipe = lease.pipe();
        if name == "index.m3u8" && !self.hls.wait_ready(pipe, Duration::from_secs(15)).await {
            return Err(Error::Unavailable(format!(
                "HLS output is not available: {}",
                pipe.last_error().trim()
            )));
        }
        Ok(pipe.dir.join(name))
    }

    pub async fn shutdown(&self) {
        self.sessions.lock().unwrap().clear();
        self.hub.shutdown();
        self.hls.shutdown().await;
    }
}

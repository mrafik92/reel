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
use tokio_util::sync::CancellationToken;

use crate::{
    ChannelHub, Error, Holder, Result, Subscription,
    hls::{AudioMode, HlsLease, HlsManager, HlsOptions, VideoMode, valid_hls_file},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveMode {
    /// Raw MPEG-TS for native players (zero CPU).
    Passthrough,
    /// fMP4 HLS for browsers.
    Hls,
    /// The client plays the provider's stream itself. Reel only holds the provider connection
    /// slot (so the budget stays honest) and carries no video.
    Direct,
}

#[derive(Debug, Clone, Serialize)]
pub struct HlsDetail {
    pub video: VideoMode,
    pub audio: AudioMode,
    /// `copy` or the video encoder backend (`vaapi`, `qsv`, `nvenc`, `videotoolbox`, `software`).
    pub encoder: &'static str,
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
    /// Provider URL a [`LiveMode::Direct`] client plays. Contains the provider login: never
    /// serialized, only handed to the session's owner by the tune call.
    #[serde(skip)]
    pub direct_url: Option<String>,
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
    /// Provider source whose connection budget this session counts against (limited sources only).
    source: Option<i64>,
    /// Cancelled when the session is stopped/taken over: ends a running passthrough stream.
    cancel: CancellationToken,
    /// A player has connected at least once (passthrough).
    connected: std::sync::atomic::AtomicBool,
    /// Direct sessions: the provider connection this session holds while its client plays.
    _slot: Option<crate::hub::ExternalSlot>,
    direct_url: Option<String>,
}

/// HLS sessions with no playlist/segment request for this long are closed (slot released).
const HLS_IDLE: Duration = Duration::from_secs(15);
/// A direct session is closed when its client has not sent a keepalive for this long (or the
/// manager's idle timeout, if shorter). The
/// server never sees the video, so the heartbeat is the only sign the viewer is still there.
pub const DIRECT_IDLE: Duration = Duration::from_secs(45);
/// A passthrough session whose player disconnected is closed after this (reconnect hiccups).
const PASSTHROUGH_GRACE: Duration = Duration::from_secs(2);

/// A VOD playback holding one connection of a provider (shares the live budget).
pub struct VodLease {
    pub id: String,
    pub user_id: i64,
    pub source_id: i64,
    pub title: String,
    pub started_at: i64,
    cancel: CancellationToken,
    /// Playback session this lease belongs to (set once the session exists).
    session: Mutex<Option<String>>,
    on_release: Arc<dyn Fn(Option<String>) + Send + Sync>,
    _slot: crate::hub::ExternalSlot,
}

impl VodLease {
    /// Cancelled when the lease is taken over / released.
    pub fn cancelled(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn session_id(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
}

pub struct LiveManager {
    hub: ChannelHub,
    hls: HlsManager,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    vod: Mutex<HashMap<String, Arc<VodLease>>>,
    idle: Duration,
}

/// Anything that holds a provider connection on behalf of a user.
enum Held {
    Live(Arc<Session>),
    Vod(Arc<VodLease>),
}

impl Held {
    fn started_at(&self) -> i64 {
        match self {
            Held::Live(s) => s.started_at,
            Held::Vod(v) => v.started_at,
        }
    }
    fn holder(&self) -> Holder {
        match self {
            Held::Live(s) => Holder {
                session_id: s.id.clone(),
                user_id: s.user_id,
                channel_id: s.channel_id,
                channel_name: s.name.clone(),
                since: s.started_at,
            },
            Held::Vod(v) => Holder {
                session_id: v.session_id().unwrap_or_else(|| v.id.clone()),
                user_id: v.user_id,
                channel_id: 0,
                channel_name: v.title.clone(),
                since: v.started_at,
            },
        }
    }
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
            vod: Mutex::default(),
            idle: session_idle,
        })
    }

    /// Use this hardware encoder for video transcodes (falls back to software at runtime if the
    /// hardware path fails).
    pub fn set_hwaccel(&self, hw: reel_playback::HwAccel) {
        self.hls.set_hwaccel(hw);
    }

    /// `(configured encoder name, fell back to software after a hardware failure)`.
    pub fn hw_status(&self) -> (&'static str, bool) {
        self.hls.hw_status()
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
                let keep = v.ts_active.load(Relaxed) > 0 || {
                    let seen = v.last_seen.lock().unwrap().elapsed();
                    match v.mode {
                        LiveMode::Hls => seen < idle.min(HLS_IDLE),
                        LiveMode::Direct => seen < idle.min(DIRECT_IDLE),
                        LiveMode::Passthrough if v.connected.load(Relaxed) => {
                            seen < PASSTHROUGH_GRACE.min(idle)
                        }
                        LiveMode::Passthrough => seen < idle,
                    }
                };
                if !keep {
                    v.cancel.cancel();
                }
                keep
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
        self.tune_ex(user_id, channel_id, mode, opts, false).await
    }

    /// [`Self::tune`] with the budget policy spelled out:
    /// * budget exhausted by the *same user's* other sessions → the oldest of them is stopped and
    ///   the slot handed over (newest wins, never an error for your own streams);
    /// * held by other users → [`Error::InUseByOthers`], unless `takeover` (callers pass it only
    ///   for admins), which stops those sessions.
    pub async fn tune_ex(
        &self,
        user_id: i64,
        channel_id: i64,
        mode: LiveMode,
        opts: &HlsOptions,
        takeover: bool,
    ) -> Result<LiveSessionInfo> {
        if mode == LiveMode::Direct {
            return self.tune_direct(user_id, channel_id, takeover).await;
        }
        let source = self.make_room(user_id, channel_id, takeover).await?;
        let (pre, lease, name) = match mode {
            LiveMode::Passthrough => {
                let sub = self.hub.subscribe(channel_id, Some(user_id)).await?;
                // surface a refused provider connection now, not as an empty stream later
                sub.wait_ready(Duration::from_secs(6)).await?;
                let name = sub.pipe_metrics().name;
                (Some(sub), None, name)
            }
            LiveMode::Direct => unreachable!("handled by tune_direct"),
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
            source,
            cancel: CancellationToken::new(),
            connected: std::sync::atomic::AtomicBool::new(false),
            _slot: None,
            direct_url: None,
        });
        let info = self.info_of(&s);
        self.sessions.lock().unwrap().insert(s.id.clone(), s);
        Ok(info)
    }

    /// [`LiveMode::Direct`]: the client will open its own connection to the provider, so this
    /// claims a full provider slot (never shares a hub pipe: that would be a second connection)
    /// under the usual policy and returns the provider URL. The slot is held until the session
    /// is stopped or the client stops sending [`Self::keepalive`].
    async fn tune_direct(
        &self,
        user_id: i64,
        channel_id: i64,
        takeover: bool,
    ) -> Result<LiveSessionInfo> {
        let spec = self
            .hub
            .channel_spec(channel_id)
            .await?
            .ok_or_else(|| Error::NotFound("channel".into()))?;
        let cand = spec
            .candidates
            .first()
            .ok_or_else(|| Error::NotFound("channel".into()))?;
        let max = cand.max_connections.filter(|m| *m > 0);
        if let Some(max) = max {
            self.make_room_for(user_id, cand.source_id, max, i64::MIN, takeover)
                .await?;
        }
        let slot = self.hub.external_slot(cand.source_id, max)?;
        let s = Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            user_id,
            channel_id,
            name: spec.name.clone(),
            mode: LiveMode::Direct,
            started_at: now_unix(),
            last_seen: Mutex::new(Instant::now()),
            ts_active: AtomicUsize::new(0),
            pre: Mutex::new(None),
            lease: None,
            source: max.map(|_| cand.source_id),
            cancel: CancellationToken::new(),
            connected: std::sync::atomic::AtomicBool::new(false),
            _slot: Some(slot),
            direct_url: Some(cand.url.clone()),
        });
        let info = self.info_of(&s);
        self.sessions.lock().unwrap().insert(s.id.clone(), s);
        Ok(info)
    }

    /// Heartbeat of a direct session. `false` if the session is gone (stopped, taken over or
    /// timed out), which tells the client to re-tune.
    pub fn keepalive(&self, id: &str) -> bool {
        match self.get(id) {
            Ok(s) => {
                *s.last_seen.lock().unwrap() = Instant::now();
                true
            }
            Err(_) => false,
        }
    }

    /// Wait (bounded) until a provider slot is free or evictable.
    async fn wait_slot(&self, source: i64, max: u32, channel: i64, up_to: Duration) -> bool {
        let end = Instant::now() + up_to;
        loop {
            if self.hub.slot_available(source, max, channel) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Stop one session right now: its stream ends, its upstream/packager is released.
    async fn release_session(&self, s: Arc<Session>) {
        s.cancel.cancel();
        self.sessions.lock().unwrap().remove(&s.id);
        drop(s);
        self.hls.release_unwatched(i64::MIN).await;
    }

    /// Stop one holder right now (stream cut, upstream/packager/VOD session released).
    async fn release_held(&self, h: Held) {
        match h {
            Held::Live(s) => self.release_session(s).await,
            Held::Vod(v) => self.release_vod(&v),
        }
    }

    fn release_vod(&self, v: &Arc<VodLease>) {
        if self.vod.lock().unwrap().remove(&v.id).is_some() {
            v.cancel.cancel();
            (v.on_release)(v.session_id());
        }
    }

    /// Make sure `channel_id` can open a provider connection (see [`Self::tune_ex`]); returns the
    /// limited source it will count against.
    async fn make_room(
        &self,
        user_id: i64,
        channel_id: i64,
        takeover: bool,
    ) -> Result<Option<i64>> {
        let Some((source, max)) = self.hub.channel_budget(channel_id).await? else {
            return Ok(None);
        };
        if self.hub.has_pipe(channel_id) {
            return Ok(Some(source));
        }
        self.make_room_for(user_id, source, max, channel_id, takeover)
            .await?;
        Ok(Some(source))
    }

    /// The budget policy, shared by live tunes and VOD plays (`except` = the channel about to be
    /// opened, `i64::MIN` for VOD): same user → newest wins; other users → [`Error::InUseByOthers`]
    /// unless `takeover`.
    async fn make_room_for(
        &self,
        user_id: i64,
        source: i64,
        max: u32,
        except: i64,
        takeover: bool,
    ) -> Result<()> {
        if self.hub.slot_available(source, max, except) {
            return Ok(());
        }
        // idle packager outputs (nobody requesting segments) hold slots through their grace
        self.hls.release_unwatched(except).await;
        if self
            .wait_slot(source, max, except, Duration::from_millis(300))
            .await
        {
            return Ok(());
        }
        let on_source = |me: &Self, mine: bool| -> Vec<Held> {
            let mut v: Vec<Held> = me
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter(|s| s.source == Some(source) && (s.user_id == user_id) == mine)
                .cloned()
                .map(Held::Live)
                .collect();
            v.extend(
                me.vod
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|l| l.source_id == source && (l.user_id == user_id) == mine)
                    .cloned()
                    .map(Held::Vod),
            );
            v.sort_by_key(Held::started_at);
            v
        };
        // 1. newest wins: the user's own sessions (any device), oldest first
        for victim in on_source(self, true) {
            tracing::info!(
                user_id,
                "provider limit reached: stopping the user's older stream so the new one gets the slot"
            );
            self.release_held(victim).await;
            if self
                .wait_slot(source, max, except, Duration::from_secs(1))
                .await
            {
                return Ok(());
            }
        }
        // 2. everyone else
        let others = on_source(self, false);
        if others.is_empty() {
            // holders without a session (e.g. a raw subscriber): the hub decides
            return Ok(());
        }
        if takeover {
            for victim in others {
                tracing::warn!(user_id, "admin takeover: stopping another user's stream");
                self.release_held(victim).await;
                if self
                    .wait_slot(source, max, except, Duration::from_secs(1))
                    .await
                {
                    return Ok(());
                }
            }
            return Ok(());
        }
        Err(Error::InUseByOthers {
            source_id: source,
            max,
            holders: others.iter().map(Held::holder).collect(),
        })
    }

    /// Claim one provider connection for VOD playback by `user_id` (same policy as live tunes).
    /// `on_release(session_id)` is called when the lease is taken over or released early so the
    /// caller can stop the playback session. Sources without a limit still get a (free) lease so
    /// the stream shows up in holder lists.
    pub async fn vod_claim(
        &self,
        user_id: i64,
        source_id: i64,
        max: Option<u32>,
        title: &str,
        takeover: bool,
        on_release: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> Result<Arc<VodLease>> {
        if let Some(max) = max.filter(|m| *m > 0) {
            self.make_room_for(user_id, source_id, max, i64::MIN, takeover)
                .await?;
        }
        let lease = Arc::new(VodLease {
            id: uuid::Uuid::new_v4().to_string(),
            user_id,
            source_id,
            title: title.to_string(),
            started_at: now_unix(),
            cancel: CancellationToken::new(),
            session: Mutex::new(None),
            on_release,
            _slot: self.hub.external_slot(source_id, max)?,
        });
        self.vod
            .lock()
            .unwrap()
            .insert(lease.id.clone(), lease.clone());
        Ok(lease)
    }

    /// Attach the playback session id to a lease.
    pub fn vod_attach(&self, lease: &VodLease, session_id: &str) {
        *lease.session.lock().unwrap() = Some(session_id.to_string());
    }

    /// Drop a lease without calling the release callback (the caller stops the session itself).
    pub fn vod_drop(&self, lease_id: &str) {
        if let Some(v) = self.vod.lock().unwrap().remove(lease_id) {
            v.cancel.cancel();
        }
    }

    /// Release the lease of a playback session (stop / end of session).
    pub fn vod_release_session(&self, session_id: &str) -> bool {
        let found = self
            .vod
            .lock()
            .unwrap()
            .values()
            .find(|v| v.session_id().as_deref() == Some(session_id))
            .cloned();
        match found {
            Some(v) => {
                self.vod.lock().unwrap().remove(&v.id);
                v.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Release leases whose playback session no longer exists (idle-reaped or crashed).
    pub fn vod_reap(&self, alive: &std::collections::HashSet<String>) -> usize {
        let dead: Vec<Arc<VodLease>> = self
            .vod
            .lock()
            .unwrap()
            .values()
            .filter(|v| match v.session_id() {
                Some(s) => !alive.contains(&s),
                // claimed but never attached: abandoned after a minute
                None => now_unix() - v.started_at > 60,
            })
            .cloned()
            .collect();
        let n = dead.len();
        for v in dead {
            self.vod.lock().unwrap().remove(&v.id);
            v.cancel.cancel();
        }
        n
    }

    pub fn vod_lease_count(&self) -> usize {
        self.vod.lock().unwrap().len()
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
                encoder: l.pipe().encoder,
            }),
            direct_url: s.direct_url.clone(),
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
        match self.sessions.lock().unwrap().remove(id) {
            Some(s) => {
                s.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Stop a session and give its provider slot back immediately (no HLS grace).
    pub async fn stop_and_release(&self, id: &str) -> bool {
        let s = self.sessions.lock().unwrap().remove(id);
        let Some(s) = s else { return false };
        s.cancel.cancel();
        let limited = s.source.is_some();
        drop(s);
        if limited {
            self.hls.release_unwatched(i64::MIN).await;
        }
        true
    }

    /// Stop the user's oldest live session (for per-user stream limits: newest wins).
    pub async fn stop_oldest_of_user(&self, user_id: i64) -> bool {
        let oldest = {
            let s = self.sessions.lock().unwrap();
            s.values()
                .filter(|v| v.user_id == user_id)
                .min_by_key(|v| v.started_at)
                .map(|v| v.id.clone())
        };
        match oldest {
            Some(id) => self.stop_and_release(&id).await,
            None => false,
        }
    }

    /// Raw MPEG-TS byte stream of a passthrough session. Dropping it frees the viewer.
    pub async fn ts_stream(
        &self,
        id: &str,
    ) -> Result<impl Stream<Item = io::Result<Bytes>> + Send + use<>> {
        let s = self.get(id)?;
        if s.mode != LiveMode::Passthrough {
            return Err(Error::Invalid(
                "this session is not passthrough; use its own URL".into(),
            ));
        }
        let pre = s.pre.lock().unwrap().take();
        let sub = match pre {
            Some(p) => p,
            None => self.hub.subscribe(s.channel_id, Some(s.user_id)).await?,
        };
        s.ts_active.fetch_add(1, Relaxed);
        s.connected.store(true, Relaxed);
        let cancel = s.cancel.clone();
        let guard = ActiveGuard(s);
        let st = futures_util::stream::unfold((sub, guard, cancel), |(mut sub, g, c)| async move {
            tokio::select! {
                _ = c.cancelled() => None,
                b = sub.next() => b.map(|b| (Ok(b), (sub, g, c))),
            }
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
            .ok_or_else(|| Error::Invalid("this session has no HLS output".into()))?;
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

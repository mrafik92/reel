//! The channel hub: one shared upstream connection per channel, fanned out to any number of
//! viewers through a lossless-for-fast / skip-for-slow broadcast, with a GOP cache for instant
//! starts, a per-source connection budget, warm channels, failover and health metrics.
//!
//! Data plane (hot path): upstream bytes → 188-byte alignment → keyframe-aware chunking →
//! `Pipe::publish` (GOP cache update + `broadcast::send`). The upstream task never awaits a
//! viewer: a viewer that falls behind is re-synced to the latest keyframe by the receiving side.

use std::{
    collections::{HashMap, VecDeque},
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use rand::Rng;
use serde::Serialize;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use reel_playback::upstream::{Lease, UpstreamOpts, UpstreamProxy};

use crate::{
    Error, Result,
    ts::{PKT, SYNC, StreamInfo, TsAnalyzer},
};

/// An HLS upstream's proxy tokens die this long after ffmpeg last used them (it reloads a live
/// playlist every few seconds; the lease is also revoked as soon as the connection ends).
const LIVE_TOKEN_IDLE: Duration = Duration::from_secs(120);

// ───────────────────────── configuration & resolution ─────────────────────────

/// One upstream URL a channel can be pulled from (the primary or a failover alias).
#[derive(Debug, Clone)]
pub struct Candidate {
    /// `channels.id` of this URL's channel row.
    pub channel_id: i64,
    pub source_id: i64,
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    /// Provider connection limit of the source (`None` / 0 = unlimited).
    pub max_connections: Option<u32>,
}

/// Everything the hub needs to pull a channel. `candidates[0]` is the primary.
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub channel_id: i64,
    pub name: String,
    pub candidates: Vec<Candidate>,
}

#[async_trait]
pub trait ChannelResolver: Send + Sync {
    async fn resolve(&self, channel_id: i64) -> Result<Option<ChannelSpec>>;
}

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Keep the upstream open this long after the last viewer left (instant re-zap).
    pub grace: Duration,
    /// How long the previously watched channel of a user stays warm.
    pub warm_ttl: Duration,
    /// No bytes for this long ⇒ the connection is considered stalled.
    pub stall_after: Duration,
    /// Consecutive failed connections before switching to the next alias.
    pub failover_after: u32,
    /// A connection that delivered data for this long resets the failure counter.
    pub healthy_after: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub connect_timeout: Duration,
    /// Broadcast capacity in chunks; slower viewers skip to the latest keyframe.
    pub ring_chunks: usize,
    /// GOP cache cap (bytes); over it the cache is dropped until the next keyframe.
    pub gop_max_bytes: usize,
    pub reap_interval: Duration,
    pub default_user_agent: String,
    pub ffmpeg_path: String,
    /// Allow upstream URLs on private networks (default true: LAN tuners are common).
    pub allow_private_upstream: bool,
    /// Providers with a connection limit release slots with a lag: a fresh channel whose first
    /// connection is refused (HTTP 503/429, connection refused) is retried every
    /// `busy_retry_step` for this long before the pipe gives up with a clear error.
    pub busy_retry_window: Duration,
    pub busy_retry_step: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(15),
            warm_ttl: Duration::from_secs(120),
            stall_after: Duration::from_secs(8),
            failover_after: 2,
            healthy_after: Duration::from_secs(30),
            backoff_base: Duration::from_millis(500),
            backoff_max: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            ring_chunks: 1024,
            gop_max_bytes: 24 * 1024 * 1024,
            reap_interval: Duration::from_millis(250),
            default_user_agent: "VLC/3.0.20 LibVLC/3.0.20".into(),
            ffmpeg_path: "ffmpeg".into(),
            allow_private_upstream: true,
            busy_retry_window: Duration::from_secs(3),
            busy_retry_step: Duration::from_millis(250),
        }
    }
}

// ───────────────────────── chunks, ingest ─────────────────────────

/// A run of whole TS packets. `key` ⇒ it begins exactly at a keyframe (GOP start).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub bytes: Bytes,
    pub key: bool,
}

/// Aligns raw upstream bytes to 188-byte packets and cuts chunks at keyframes.
pub struct Ingest {
    carry: BytesMut,
    pub analyzer: TsAnalyzer,
    synced: bool,
    pub sync_losses: u64,
}

impl Default for Ingest {
    fn default() -> Self {
        Self::new()
    }
}

fn find_sync(buf: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j < buf.len() {
        if buf[j] == SYNC
            && (j + PKT >= buf.len() || buf[j + PKT] == SYNC)
            && (j + 2 * PKT >= buf.len() || buf[j + 2 * PKT] == SYNC)
        {
            return Some(j);
        }
        j += 1;
    }
    None
}

impl Ingest {
    pub fn new() -> Self {
        Self {
            carry: BytesMut::new(),
            analyzer: TsAnalyzer::new(),
            synced: false,
            sync_losses: 0,
        }
    }

    /// Called on reconnect: drop partial packets and per-stream state.
    pub fn reset(&mut self) {
        self.carry.clear();
        self.synced = false;
        self.analyzer.reset_stream();
    }

    /// Feed bytes; complete chunks are appended to `out`.
    pub fn feed(&mut self, data: Bytes, out: &mut Vec<Chunk>) {
        let buf: Bytes = if self.carry.is_empty() {
            data
        } else {
            self.carry.extend_from_slice(&data);
            std::mem::take(&mut self.carry).freeze()
        };
        let len = buf.len();
        let (mut start, mut i) = (0usize, 0usize);
        let mut chunk_key = false;
        loop {
            if i + PKT > len {
                break;
            }
            if buf[i] != SYNC {
                if i > start {
                    out.push(Chunk {
                        bytes: buf.slice(start..i),
                        key: chunk_key,
                    });
                }
                chunk_key = false;
                if self.synced {
                    self.sync_losses += 1;
                    self.synced = false;
                }
                match find_sync(&buf, i) {
                    Some(j) => {
                        i = j;
                        start = j;
                        self.synced = true;
                        continue;
                    }
                    None => {
                        // keep a possible partial packet start at the very end
                        let keep_from = len.saturating_sub(PKT - 1).max(i);
                        self.carry.extend_from_slice(&buf[keep_from..]);
                        return;
                    }
                }
            }
            if !self.synced {
                // first packet(s): confirm alignment before trusting it
                match find_sync(&buf, i) {
                    Some(j) if j == i => self.synced = true,
                    Some(j) => {
                        i = j;
                        start = j;
                        self.synced = true;
                        continue;
                    }
                    None => {
                        self.carry.clear();
                        return;
                    }
                }
            }
            let ev = self.analyzer.analyze(&buf.slice(i..i + PKT));
            if ev.keyframe {
                if i > start {
                    out.push(Chunk {
                        bytes: buf.slice(start..i),
                        key: chunk_key,
                    });
                    start = i;
                }
                chunk_key = true;
            }
            i += PKT;
        }
        if i > start {
            out.push(Chunk {
                bytes: buf.slice(start..i),
                key: chunk_key,
            });
        }
        if i < len {
            self.carry.extend_from_slice(&buf[i..]);
        }
    }
}

// ───────────────────────── pipe (one channel) ─────────────────────────

#[derive(Default)]
struct PipeData {
    gop: VecDeque<Chunk>,
    gop_bytes: usize,
    has_key: bool,
    pat: Option<Bytes>,
    pmt: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipeState {
    Connecting,
    Live,
    Reconnecting,
}

const UNSET: u64 = u64::MAX;

struct Idle {
    keep_until: Instant,
    last_viewed: Instant,
}

pub struct Pipe {
    pub id: i64,
    pub name: String,
    tx: broadcast::Sender<Chunk>,
    data: Mutex<PipeData>,
    viewers: AtomicUsize,
    idle: Mutex<Idle>,
    grace: Duration,
    pinned: AtomicBool,
    cancel: CancellationToken,
    source_id: AtomicI64,
    info_tx: watch::Sender<Option<StreamInfo>>,
    created: Instant,
    // metrics
    bytes_total: AtomicU64,
    rate_kbps: AtomicU64,
    reconnects: AtomicU64,
    failovers: AtomicU64,
    cc_errors: AtomicU64,
    pts_discontinuities: AtomicU64,
    sync_losses: AtomicU64,
    keyframes: AtomicU64,
    lagged: AtomicU64,
    ttfb_ms: AtomicU64,
    ttk_ms: AtomicU64,
    active_candidate: AtomicUsize,
    live_since: Mutex<Option<Instant>>,
    state: Mutex<PipeState>,
    last_error: Mutex<Option<String>>,
    active_url_host: Mutex<Option<String>>,
    /// Provider connection limit of the source (0 = unlimited / unknown).
    max_conn: u32,
    /// Set when the pipe gave up for good (the provider kept refusing the connection).
    fatal: Mutex<Option<String>>,
}

impl Pipe {
    fn new(spec: &ChannelSpec, cfg: &HubConfig, source_id: i64, keep: Duration) -> Arc<Self> {
        let (tx, _) = broadcast::channel(cfg.ring_chunks.max(16));
        let now = Instant::now();
        let max_conn = spec
            .candidates
            .first()
            .and_then(|c| c.max_connections)
            .unwrap_or(0);
        // A single-connection provider cannot afford warm/grace channels: release at once.
        let single = max_conn == 1;
        let keep = if single { Duration::ZERO } else { keep };
        Arc::new(Self {
            id: spec.channel_id,
            name: spec.name.clone(),
            tx,
            data: Mutex::default(),
            viewers: AtomicUsize::new(0),
            idle: Mutex::new(Idle {
                keep_until: now + keep,
                last_viewed: now,
            }),
            grace: if single { Duration::ZERO } else { cfg.grace },
            pinned: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            source_id: AtomicI64::new(source_id),
            info_tx: watch::channel(None).0,
            created: now,
            bytes_total: AtomicU64::new(0),
            rate_kbps: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            failovers: AtomicU64::new(0),
            cc_errors: AtomicU64::new(0),
            pts_discontinuities: AtomicU64::new(0),
            sync_losses: AtomicU64::new(0),
            keyframes: AtomicU64::new(0),
            lagged: AtomicU64::new(0),
            ttfb_ms: AtomicU64::new(UNSET),
            ttk_ms: AtomicU64::new(UNSET),
            active_candidate: AtomicUsize::new(0),
            live_since: Mutex::new(None),
            state: Mutex::new(PipeState::Connecting),
            last_error: Mutex::new(None),
            active_url_host: Mutex::new(None),
            max_conn,
            fatal: Mutex::new(None),
        })
    }

    pub fn viewers(&self) -> usize {
        self.viewers.load(Relaxed)
    }

    fn single_connection(&self) -> bool {
        self.max_conn == 1
    }

    fn fatal(&self) -> Option<String> {
        self.fatal.lock().unwrap().clone()
    }

    fn source(&self) -> i64 {
        self.source_id.load(Relaxed)
    }

    fn last_viewed(&self) -> Instant {
        self.idle.lock().unwrap().last_viewed
    }

    fn extend_keep(&self, until: Instant) {
        let mut i = self.idle.lock().unwrap();
        if until > i.keep_until {
            i.keep_until = until;
        }
    }

    fn set_state(&self, s: PipeState) {
        *self.state.lock().unwrap() = s;
    }

    /// Stream layout once the PMT has been seen.
    pub fn stream_info(&self) -> Option<StreamInfo> {
        self.info_tx.borrow().clone()
    }

    /// Wait (bounded) for the PMT.
    pub async fn wait_info(&self, max: Duration) -> Option<StreamInfo> {
        let mut rx = self.info_tx.subscribe();
        let r = tokio::time::timeout(max, async {
            loop {
                if let Some(i) = rx.borrow_and_update().clone() {
                    return Some(i);
                }
                tokio::select! {
                    _ = self.cancel.cancelled() => return None,
                    c = rx.changed() => if c.is_err() {
                        return None;
                    },
                }
            }
        })
        .await;
        r.ok().flatten()
    }

    fn set_psi(&self, pat: Bytes, pmt: Bytes) {
        let mut d = self.data.lock().unwrap();
        d.pat = Some(pat);
        d.pmt = Some(pmt);
    }

    /// Hot path: update the GOP cache and wake subscribers. Never blocks on viewers.
    fn publish(&self, chunk: Chunk, gop_max: usize) {
        self.bytes_total
            .fetch_add(chunk.bytes.len() as u64, Relaxed);
        let mut d = self.data.lock().unwrap();
        if chunk.key {
            d.gop.clear();
            d.gop_bytes = 0;
            d.has_key = true;
        }
        if d.has_key {
            d.gop_bytes += chunk.bytes.len();
            d.gop.push_back(chunk.clone());
            if d.gop_bytes > gop_max {
                d.gop.clear();
                d.gop_bytes = 0;
                d.has_key = false;
            }
        }
        let _ = self.tx.send(chunk);
    }

    fn clear_gop(&self) {
        let mut d = self.data.lock().unwrap();
        d.gop.clear();
        d.gop_bytes = 0;
        d.has_key = false;
    }

    /// Atomically: snapshot of PAT/PMT + GOP and a receiver positioned right after it.
    fn snapshot(&self) -> (VecDeque<Bytes>, broadcast::Receiver<Chunk>, bool) {
        let d = self.data.lock().unwrap();
        let mut initial = VecDeque::new();
        if d.has_key {
            if let (Some(a), Some(b)) = (&d.pat, &d.pmt) {
                initial.push_back(a.clone());
                initial.push_back(b.clone());
            }
            initial.extend(d.gop.iter().map(|c| c.bytes.clone()));
        }
        (initial, self.tx.subscribe(), !d.has_key)
    }

    fn psi(&self) -> Vec<Bytes> {
        let d = self.data.lock().unwrap();
        match (&d.pat, &d.pmt) {
            (Some(a), Some(b)) => vec![a.clone(), b.clone()],
            _ => vec![],
        }
    }

    pub fn metrics(&self) -> ChannelMetrics {
        let live_since = *self.live_since.lock().unwrap();
        let (v, a) = match self.stream_info() {
            Some(i) => (
                i.video.map(|v| v.name().to_string()),
                i.audio.first().map(|a| a.1.name().to_string()),
            ),
            None => (None, None),
        };
        let opt = |x: u64| (x != UNSET).then_some(x);
        ChannelMetrics {
            channel_id: self.id,
            name: self.name.clone(),
            source_id: self.source(),
            state: *self.state.lock().unwrap(),
            viewers: self.viewers(),
            warm: self.viewers() == 0,
            pinned: self.pinned.load(Relaxed),
            uptime_s: self.created.elapsed().as_secs(),
            connected_s: live_since.map(|t| t.elapsed().as_secs()),
            bitrate_kbps: self.rate_kbps.load(Relaxed),
            bytes_total: self.bytes_total.load(Relaxed),
            reconnects: self.reconnects.load(Relaxed),
            failovers: self.failovers.load(Relaxed),
            active_candidate: self.active_candidate.load(Relaxed),
            cc_errors: self.cc_errors.load(Relaxed),
            pts_discontinuities: self.pts_discontinuities.load(Relaxed),
            sync_losses: self.sync_losses.load(Relaxed),
            keyframes: self.keyframes.load(Relaxed),
            viewer_resyncs: self.lagged.load(Relaxed),
            ttfb_ms: opt(self.ttfb_ms.load(Relaxed)),
            time_to_keyframe_ms: opt(self.ttk_ms.load(Relaxed)),
            gop_bytes: self.data.lock().unwrap().gop_bytes,
            video_codec: v,
            audio_codec: a,
            last_error: self.last_error.lock().unwrap().clone(),
            upstream_host: self.active_url_host.lock().unwrap().clone(),
        }
    }
}

/// Live health numbers of one channel pipe (admin dashboard).
#[derive(Debug, Clone, Serialize)]
pub struct ChannelMetrics {
    pub channel_id: i64,
    pub name: String,
    pub source_id: i64,
    pub state: PipeState,
    pub viewers: usize,
    /// No viewers: kept open for instant zapping.
    pub warm: bool,
    pub pinned: bool,
    pub uptime_s: u64,
    pub connected_s: Option<u64>,
    pub bitrate_kbps: u64,
    pub bytes_total: u64,
    pub reconnects: u64,
    pub failovers: u64,
    /// Index into the channel's candidate list (0 = primary).
    pub active_candidate: usize,
    pub cc_errors: u64,
    pub pts_discontinuities: u64,
    pub sync_losses: u64,
    pub keyframes: u64,
    /// Times a slow viewer was skipped ahead to the latest keyframe.
    pub viewer_resyncs: u64,
    /// Pipe start → first upstream byte.
    pub ttfb_ms: Option<u64>,
    /// Pipe start → first keyframe.
    pub time_to_keyframe_ms: Option<u64>,
    pub gop_bytes: usize,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub last_error: Option<String>,
    pub upstream_host: Option<String>,
}

// ───────────────────────── subscription ─────────────────────────

/// Counts a viewer on a pipe for as long as it lives.
pub struct ViewerGuard {
    pipe: Arc<Pipe>,
}

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        if self.pipe.viewers.fetch_sub(1, Relaxed) == 1 {
            let now = Instant::now();
            let mut i = self.pipe.idle.lock().unwrap();
            i.last_viewed = now;
            let until = now + self.pipe.grace;
            if until > i.keep_until {
                i.keep_until = until;
            }
        }
    }
}

/// A viewer's byte feed: PAT/PMT + the cached GOP first (instant start), then live chunks.
/// If the viewer falls behind the ring it is transparently re-synced to the latest keyframe.
pub struct Subscription {
    pipe: Arc<Pipe>,
    rx: broadcast::Receiver<Chunk>,
    initial: VecDeque<Bytes>,
    need_sync: bool,
    _guard: ViewerGuard,
}

impl Subscription {
    fn new(pipe: Arc<Pipe>) -> Self {
        pipe.viewers.fetch_add(1, Relaxed);
        pipe.idle.lock().unwrap().last_viewed = Instant::now();
        let (initial, rx, need_sync) = pipe.snapshot();
        Subscription {
            rx,
            initial,
            need_sync,
            _guard: ViewerGuard { pipe: pipe.clone() },
            pipe,
        }
    }

    pub fn channel_id(&self) -> i64 {
        self.pipe.id
    }

    pub fn pipe_metrics(&self) -> ChannelMetrics {
        self.pipe.metrics()
    }

    pub fn stream_info(&self) -> Option<StreamInfo> {
        self.pipe.stream_info()
    }

    pub async fn wait_info(&self, max: Duration) -> Option<StreamInfo> {
        self.pipe.wait_info(max).await
    }

    /// Wait (bounded) until the upstream delivers data. Errors immediately when the provider
    /// refused the connection for good (e.g. its connection limit is still in use); a plain
    /// timeout is not an error.
    pub async fn wait_ready(&self, max: Duration) -> Result<()> {
        let end = Instant::now() + max;
        loop {
            if let Some(m) = self.pipe.fatal() {
                return Err(Error::Unavailable(m));
            }
            if self.pipe.ttfb_ms.load(Relaxed) != UNSET || Instant::now() >= end {
                return Ok(());
            }
            if self.pipe.cancel.is_cancelled() {
                return Err(Error::Unavailable(
                    "the channel was closed before it delivered data".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Next piece of the stream; `None` when the pipe was shut down.
    pub async fn next(&mut self) -> Option<Bytes> {
        loop {
            if let Some(b) = self.initial.pop_front() {
                return Some(b);
            }
            let r = tokio::select! {
                biased;
                _ = self.pipe.cancel.cancelled() => return None,
                r = self.rx.recv() => r,
            };
            match r {
                Ok(c) => {
                    if self.need_sync {
                        if !c.key {
                            continue;
                        }
                        self.need_sync = false;
                        self.initial.extend(self.pipe.psi());
                        self.initial.push_back(c.bytes);
                        continue;
                    }
                    return Some(c.bytes);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    self.pipe.lagged.fetch_add(1, Relaxed);
                    debug!(
                        channel = self.pipe.id,
                        skipped = n,
                        "viewer lagged; resyncing to keyframe"
                    );
                    let (initial, rx, need_sync) = self.pipe.snapshot();
                    self.initial = initial;
                    self.rx = rx;
                    self.need_sync = need_sync;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// As a `Stream` of byte chunks (for HTTP bodies).
    pub fn into_stream(self) -> impl Stream<Item = io::Result<Bytes>> + Send {
        futures_util::stream::unfold(
            self,
            |mut s| async move { s.next().await.map(|b| (Ok(b), s)) },
        )
    }
}

// ───────────────────────── hub ─────────────────────────

struct HubState {
    pipes: HashMap<i64, Arc<Pipe>>,
    user_last: HashMap<i64, i64>,
    /// Provider connections used by things that are not channel pipes (VOD playback).
    external: HashMap<i64, usize>,
}

/// A provider connection held outside the hub (VOD): counts against the source's budget until
/// dropped.
pub struct ExternalSlot {
    inner: Arc<Inner>,
    source_id: i64,
}

impl Drop for ExternalSlot {
    fn drop(&mut self) {
        let mut st = self.inner.state.lock().unwrap();
        if let Some(n) = st.external.get_mut(&self.source_id) {
            *n = n.saturating_sub(1);
        }
    }
}

struct Inner {
    cfg: HubConfig,
    http: reqwest::Client,
    /// Loopback proxy for HLS upstreams: ffmpeg gets a `127.0.0.1` token URL, never the
    /// provider URL (credentials stay out of argv and `/proc`; every fetch is SSRF-guarded).
    proxy: UpstreamProxy,
    resolver: Arc<dyn ChannelResolver>,
    state: Mutex<HubState>,
    shutdown: CancellationToken,
    /// Per-source politeness shared with the catalog/guide calls (set by the service).
    gates: Mutex<Option<Arc<crate::gate::Gates>>>,
}

#[derive(Clone)]
pub struct ChannelHub {
    inner: Arc<Inner>,
}

impl ChannelHub {
    pub fn new(cfg: HubConfig, resolver: Arc<dyn ChannelResolver>) -> Self {
        let http = crate::net::client(cfg.allow_private_upstream);
        let allow = cfg.allow_private_upstream;
        let proxy = UpstreamProxy::new(
            http.clone(),
            Arc::new(move |u: &str| crate::net::check_url(u, allow).map_err(|e| e.to_string())),
        );
        ChannelHub {
            inner: Arc::new(Inner {
                cfg,
                http,
                proxy,
                resolver,
                state: Mutex::new(HubState {
                    pipes: HashMap::new(),
                    user_last: HashMap::new(),
                    external: HashMap::new(),
                }),
                shutdown: CancellationToken::new(),
                gates: Mutex::new(None),
            }),
        }
    }

    /// Share the per-source politeness gates: stream connects are paced and a 46x/429 answer
    /// pauses catalog/guide calls to the same provider.
    pub fn set_gates(&self, gates: Arc<crate::gate::Gates>) {
        *self.inner.gates.lock().unwrap() = Some(gates);
    }

    pub fn config(&self) -> &HubConfig {
        &self.inner.cfg
    }

    /// Upstream-proxy tokens currently alive (HLS upstreams; diagnostics / tests).
    pub fn proxy_tokens(&self) -> usize {
        self.inner.proxy.token_count()
    }

    /// Start the background reaper (tears down idle pipes after the grace period).
    pub fn spawn_reaper(&self) -> tokio::task::JoinHandle<()> {
        let hub = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(hub.inner.cfg.reap_interval);
            loop {
                tokio::select! {
                    _ = hub.inner.shutdown.cancelled() => return,
                    _ = tick.tick() => hub.reap(),
                }
            }
        })
    }

    fn reap(&self) {
        let now = Instant::now();
        let mut st = self.inner.state.lock().unwrap();
        let dead: Vec<i64> = st
            .pipes
            .values()
            .filter(|p| {
                p.viewers() == 0
                    && !p.pinned.load(Relaxed)
                    && now >= p.idle.lock().unwrap().keep_until
            })
            .map(|p| p.id)
            .collect();
        for id in dead {
            if let Some(p) = st.pipes.remove(&id) {
                info!(channel = id, name = %p.name, "closing idle channel upstream");
                p.cancel.cancel();
            }
        }
    }

    /// Stop everything.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        let mut st = self.inner.state.lock().unwrap();
        for (_, p) in st.pipes.drain() {
            p.cancel.cancel();
        }
    }

    /// Metrics of every open channel pipe.
    pub fn metrics(&self) -> Vec<ChannelMetrics> {
        let st = self.inner.state.lock().unwrap();
        let mut v: Vec<_> = st.pipes.values().map(|p| p.metrics()).collect();
        v.sort_by_key(|m| m.channel_id);
        v
    }

    /// Open upstream connections per source.
    pub fn connections_by_source(&self) -> HashMap<i64, usize> {
        let st = self.inner.state.lock().unwrap();
        let mut m = HashMap::new();
        for p in st.pipes.values() {
            *m.entry(p.source()).or_insert(0) += 1;
        }
        for (s, n) in &st.external {
            if *n > 0 {
                *m.entry(*s).or_insert(0) += *n;
            }
        }
        m
    }

    pub fn is_open(&self, channel_id: i64) -> bool {
        self.inner
            .state
            .lock()
            .unwrap()
            .pipes
            .contains_key(&channel_id)
    }

    /// Make room for one more connection on `source_id`: evict the least-recently-viewed
    /// unwatched (warm) pipe, or fail if every slot has a live viewer.
    fn ensure_slot(
        st: &mut HubState,
        source_id: i64,
        max: Option<u32>,
        requester: Option<i64>,
    ) -> Result<()> {
        let Some(max) = max.filter(|m| *m > 0) else {
            return Ok(());
        };
        let at_source: Vec<Arc<Pipe>> = st
            .pipes
            .values()
            .filter(|p| p.source() == source_id && Some(p.id) != requester)
            .cloned()
            .collect();
        let external = st.external.get(&source_id).copied().unwrap_or(0);
        if ((at_source.len() + external) as u32) < max {
            return Ok(());
        }
        // evict unwatched pipes (unpinned first, then least recently viewed) until one slot is free
        let mut victims: Vec<&Arc<Pipe>> = at_source.iter().filter(|p| p.viewers() == 0).collect();
        victims.sort_by_key(|p| (p.pinned.load(Relaxed), p.last_viewed()));
        let used = at_source.len() + external;
        let need = used + 1 - max as usize; // evictions needed for one more connection
        if victims.len() < need {
            return Err(Error::BudgetExhausted {
                source_id,
                max,
                watching: at_source.iter().map(|p| p.name.clone()).collect(),
            });
        }
        for v in victims.into_iter().take(need) {
            info!(channel = v.id, name = %v.name, source_id, "evicting warm channel to free a provider slot");
            v.cancel.cancel();
            st.pipes.remove(&v.id);
        }
        Ok(())
    }

    /// A channel's upstreams (primary first): what a client needs to play the channel without
    /// going through the hub. `None` if the channel is unknown.
    pub async fn channel_spec(&self, channel_id: i64) -> Result<Option<ChannelSpec>> {
        self.inner.resolver.resolve(channel_id).await
    }

    /// `(source id, connection limit)` of the source a new connection of the channel would use
    /// (see [`Self::prefer_free`]), if limited.
    pub async fn channel_budget(&self, channel_id: i64) -> Result<Option<(i64, u32)>> {
        let Some(mut spec) = self.inner.resolver.resolve(channel_id).await? else {
            return Ok(None);
        };
        self.prefer_free(&mut spec);
        Ok(spec.candidates.first().and_then(|c| {
            c.max_connections
                .filter(|m| *m > 0)
                .map(|m| (c.source_id, m))
        }))
    }

    /// Move the first candidate whose source has a free (or evictable) slot to the front, so a
    /// new connection uses a spare login or alias before the budget policy stops another stream
    /// (#297: several logins on one provider). Leaves the order alone when none is free.
    pub fn prefer_free(&self, spec: &mut ChannelSpec) {
        let free =
            spec.candidates
                .iter()
                .position(|c| match c.max_connections.filter(|m| *m > 0) {
                    None => true,
                    Some(m) => self.slot_available(c.source_id, m, spec.channel_id),
                });
        if let Some(i) = free.filter(|i| *i > 0) {
            spec.candidates[..=i].rotate_right(1);
        }
    }

    /// Would opening one more connection on `source_id` succeed (free slot, or an unwatched
    /// pipe that can be evicted)? `except` is the channel about to be opened.
    pub fn slot_available(&self, source_id: i64, max: u32, except: i64) -> bool {
        let st = self.inner.state.lock().unwrap();
        let at: Vec<&Arc<Pipe>> = st
            .pipes
            .values()
            .filter(|p| p.source() == source_id && p.id != except)
            .collect();
        let external = st.external.get(&source_id).copied().unwrap_or(0);
        // free after evicting every unwatched pipe
        let held = at.iter().filter(|p| p.viewers() > 0).count() + external;
        (held as u32) < max
    }

    /// Reserve one provider connection for a non-channel consumer (VOD playback). The caller
    /// must have made room first (see `LiveManager::vod_claim`).
    pub fn external_slot(&self, source_id: i64, max: Option<u32>) -> Result<ExternalSlot> {
        let mut st = self.inner.state.lock().unwrap();
        // unwatched (warm) channel pipes make way for the new holder
        Self::ensure_slot(&mut st, source_id, max, None)?;
        *st.external.entry(source_id).or_insert(0) += 1;
        Ok(ExternalSlot {
            inner: self.inner.clone(),
            source_id,
        })
    }

    /// Is a connection of this channel open right now?
    pub fn has_pipe(&self, channel_id: i64) -> bool {
        self.is_open(channel_id)
    }

    fn note_user(&self, st: &mut HubState, user: Option<i64>, channel: i64) {
        let Some(u) = user else { return };
        if let Some(prev) = st.user_last.insert(u, channel) {
            if prev != channel {
                if let Some(p) = st.pipes.get(&prev).cloned() {
                    if p.single_connection() {
                        // one provider slot: the old channel must give it up right now
                        if p.viewers() == 0 && !p.pinned.load(Relaxed) {
                            info!(
                                channel = prev,
                                "releasing the previous channel (single-connection provider)"
                            );
                            p.cancel.cancel();
                            st.pipes.remove(&prev);
                        }
                    } else {
                        // previous channel stays warm for instant "channel back"
                        p.extend_keep(Instant::now() + self.inner.cfg.warm_ttl);
                    }
                }
            }
        }
    }

    /// Get a viewer feed for `channel_id`, opening the upstream if needed. Fails with
    /// [`Error::BudgetExhausted`] when the provider connection limit is fully in use.
    pub async fn subscribe(&self, channel_id: i64, user_id: Option<i64>) -> Result<Subscription> {
        {
            let mut st = self.inner.state.lock().unwrap();
            if let Some(p) = st.pipes.get(&channel_id).cloned() {
                self.note_user(&mut st, user_id, channel_id);
                return Ok(Subscription::new(p));
            }
        }
        let mut spec = self
            .inner
            .resolver
            .resolve(channel_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("channel {channel_id}")))?;
        self.prefer_free(&mut spec);
        let mut st = self.inner.state.lock().unwrap();
        if let Some(p) = st.pipes.get(&channel_id).cloned() {
            self.note_user(&mut st, user_id, channel_id);
            return Ok(Subscription::new(p));
        }
        let pipe = self.open_pipe(&mut st, spec, true, self.inner.cfg.grace)?;
        self.note_user(&mut st, user_id, channel_id);
        Ok(Subscription::new(pipe))
    }

    fn open_pipe(
        &self,
        st: &mut HubState,
        spec: ChannelSpec,
        may_evict: bool,
        keep: Duration,
    ) -> Result<Arc<Pipe>> {
        let first = spec.candidates.first().ok_or_else(|| {
            Error::Unavailable(format!("channel {} has no stream URL", spec.name))
        })?;
        if may_evict {
            Self::ensure_slot(st, first.source_id, first.max_connections, None)?;
        } else if let Some(max) = first.max_connections.filter(|m| *m > 0) {
            let used = (st
                .pipes
                .values()
                .filter(|p| p.source() == first.source_id)
                .count()
                + st.external.get(&first.source_id).copied().unwrap_or(0))
                as u32;
            if used >= max {
                return Err(Error::Unavailable("no free provider slot".into()));
            }
        }
        let pipe = Pipe::new(&spec, &self.inner.cfg, first.source_id, keep);
        st.pipes.insert(spec.channel_id, pipe.clone());
        let inner = self.inner.clone();
        let p = pipe.clone();
        tokio::spawn(async move { upstream_loop(inner, p, spec).await });
        Ok(pipe)
    }

    /// Pre-open a channel without a viewer (warm start). Never evicts and never exceeds the
    /// provider limit. Returns whether a new connection was opened.
    pub async fn warm(&self, channel_id: i64, pin: bool) -> Result<bool> {
        {
            let st = self.inner.state.lock().unwrap();
            if let Some(p) = st.pipes.get(&channel_id) {
                if pin {
                    p.pinned.store(true, Relaxed);
                } else {
                    p.extend_keep(Instant::now() + self.inner.cfg.warm_ttl);
                }
                return Ok(false);
            }
        }
        let Some(spec) = self.inner.resolver.resolve(channel_id).await? else {
            return Ok(false);
        };
        // a single-connection provider has no slot to spare for warm channels
        if spec
            .candidates
            .first()
            .and_then(|c| c.max_connections)
            .is_some_and(|m| m == 1)
        {
            return Ok(false);
        }
        let mut st = self.inner.state.lock().unwrap();
        if st.pipes.contains_key(&channel_id) {
            return Ok(false);
        }
        match self.open_pipe(&mut st, spec, false, self.inner.cfg.warm_ttl) {
            Ok(p) => {
                p.pinned.store(pin, Relaxed);
                Ok(true)
            }
            Err(Error::Unavailable(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Replace the set of pinned (always-warm) channels; un-pinned ones fall back to the grace.
    pub fn set_pinned(&self, keep: &std::collections::HashSet<i64>) {
        let st = self.inner.state.lock().unwrap();
        for p in st.pipes.values() {
            let pin = keep.contains(&p.id);
            if p.pinned.swap(pin, Relaxed) && !pin {
                p.extend_keep(Instant::now() + self.inner.cfg.grace);
            }
        }
    }
}

// ───────────────────────── upstream ─────────────────────────

fn jitter(base: Duration, attempt: u32, max: Duration) -> Duration {
    let d = base.saturating_mul(1u32 << attempt.min(6)).min(max);
    let f: f64 = rand::thread_rng().gen_range(0.75..1.25);
    d.mul_f64(f)
}

type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

struct Upstream {
    stream: ByteStream,
    /// Keeps an ffmpeg child alive (and kills it on drop) for HLS-only upstreams.
    _child: Option<tokio::process::Child>,
    /// The child's proxy tokens; dropped after the child (field order), revoking them.
    _lease: Option<Lease>,
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

async fn open_upstream(inner: &Inner, c: &Candidate) -> std::result::Result<Upstream, String> {
    let ua = c
        .user_agent
        .clone()
        .unwrap_or_else(|| inner.cfg.default_user_agent.clone());
    crate::net::check_url(&c.url, inner.cfg.allow_private_upstream)
        .map_err(|e| crate::net::redact_text(&e.to_string()))?;
    let looks_hls = reqwest::Url::parse(&c.url)
        .map(|u| u.path().to_ascii_lowercase().ends_with(".m3u8"))
        .unwrap_or(false);
    if looks_hls {
        return open_ffmpeg(inner, c, &ua);
    }
    let mut req = inner
        .http
        .get(&c.url)
        .header("User-Agent", &ua)
        .header("Accept", "*/*");
    if let Some(r) = &c.referrer {
        req = req.header("Referer", r);
    }
    let gate = inner
        .gates
        .lock()
        .unwrap()
        .as_ref()
        .map(|g| g.get(c.source_id));
    if let Some(g) = &gate {
        // pace only: a viewer is waiting, so never refuse
        let _ = g.acquire(Duration::from_secs(2), false).await;
    }
    let resp = tokio::time::timeout(inner.cfg.connect_timeout, req.send())
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| format!("connect failed: {}", e.without_url()))?;
    if let Some(g) = &gate {
        let code = resp.status().as_u16();
        // 5xx on a stream is slot lag, not abuse protection
        if code == 429 || (460..470).contains(&code) || resp.status().is_success() {
            g.report(code, crate::gate::retry_after(&resp));
        }
    }
    if !resp.status().is_success() {
        return Err(format!("provider answered HTTP {}", resp.status().as_u16()));
    }
    let hls = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("mpegurl")
        });
    if hls {
        drop(resp);
        return open_ffmpeg(inner, c, &ua);
    }
    Ok(Upstream {
        stream: Box::pin(resp.bytes_stream().map(|r| r.map_err(io::Error::other))),
        _child: None,
        _lease: None,
    })
}

/// HLS providers: let ffmpeg pull the playlist and emit one continuous MPEG-TS on stdout.
///
/// ffmpeg only ever sees a loopback URL of the hub's [`UpstreamProxy`]: the provider URL (with its
/// login), user agent and referer stay in this process, and every playlist, segment and key is
/// fetched through the SSRF-guarded client (DNS and redirects checked per request).
fn open_ffmpeg(inner: &Inner, c: &Candidate, ua: &str) -> std::result::Result<Upstream, String> {
    use std::process::Stdio;
    let opts = UpstreamOpts {
        user_agent: ua.to_string(),
        referer: c.referrer.clone(),
        idle: Some(LIVE_TOKEN_IDLE),
        exclusive: false,
    };
    let lease = inner
        .proxy
        .lease_with(&c.url, &opts, LIVE_TOKEN_IDLE)
        .map_err(|e| crate::net::redact_text(&e.to_string()))?;
    let mut cmd = tokio::process::Command::new(&inner.cfg.ffmpeg_path);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        // loopback proxy only: a playlist can make ffmpeg neither read local files nor reach
        // another host (the proxy rewrites every URI to a token and drops the rest); `crypto`
        // wraps AES-128 segments, whose keys come through the proxy too
        "-protocol_whitelist",
        "http,tcp,crypto",
        "-i",
        lease.url(),
        "-c",
        "copy",
        "-f",
        "mpegts",
        "pipe:1",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))?;
    let out = child.stdout.take().ok_or("no ffmpeg stdout")?;
    Ok(Upstream {
        stream: Box::pin(tokio_util::io::ReaderStream::with_capacity(out, 64 * 1024)),
        _child: Some(child),
        _lease: Some(lease),
    })
}

/// Errors that mean "the provider has no free slot (yet)" rather than "the channel is broken".
fn is_busy_err(err: &str) -> bool {
    [
        "HTTP 503",
        "HTTP 429",
        "HTTP 502",
        "HTTP 509",
        "HTTP 504",
        "connect failed",
    ]
    .iter()
    .any(|p| err.contains(p))
}

enum ConnEnd {
    Cancelled,
    Failed { err: String, flowed: Duration },
}

async fn run_connection(
    inner: &Inner,
    pipe: &Arc<Pipe>,
    cand: &Candidate,
    ingest: &mut Ingest,
) -> ConnEnd {
    pipe.set_state(PipeState::Connecting);
    *pipe.active_url_host.lock().unwrap() = host_of(&cand.url);
    let mut up = tokio::select! {
        _ = pipe.cancel.cancelled() => return ConnEnd::Cancelled,
        r = open_upstream(inner, cand) => match r {
            Ok(u) => u,
            Err(e) => return ConnEnd::Failed { err: e, flowed: Duration::ZERO },
        },
    };
    ingest.reset();
    let (mut win_start, mut win_bytes) = (Instant::now(), 0u64);
    let mut first_data: Option<Instant> = None;
    let mut chunks: Vec<Chunk> = Vec::with_capacity(8);
    loop {
        let item = tokio::select! {
            biased;
            _ = pipe.cancel.cancelled() => return ConnEnd::Cancelled,
            r = tokio::time::timeout(inner.cfg.stall_after, up.stream.next()) => r,
        };
        let flowed = first_data.map_or(Duration::ZERO, |t| t.elapsed());
        let bytes = match item {
            Err(_) => {
                return ConnEnd::Failed {
                    err: format!(
                        "stalled: no data for {:.1}s",
                        inner.cfg.stall_after.as_secs_f32()
                    ),
                    flowed,
                };
            }
            Ok(None) => {
                return ConnEnd::Failed {
                    err: "upstream closed the stream".into(),
                    flowed,
                };
            }
            Ok(Some(Err(e))) => {
                return ConnEnd::Failed {
                    err: format!("read error: {e}"),
                    flowed,
                };
            }
            Ok(Some(Ok(b))) => b,
        };
        if bytes.is_empty() {
            continue;
        }
        if first_data.is_none() {
            first_data = Some(Instant::now());
            pipe.set_state(PipeState::Live);
            *pipe.live_since.lock().unwrap() = Some(Instant::now());
            if pipe.ttfb_ms.load(Relaxed) == UNSET {
                pipe.ttfb_ms
                    .store(pipe.created.elapsed().as_millis() as u64, Relaxed);
            }
        }
        win_bytes += bytes.len() as u64;
        if win_start.elapsed() >= Duration::from_secs(1) {
            let kbps = win_bytes * 8 / 1000 * 1000 / win_start.elapsed().as_millis().max(1) as u64;
            pipe.rate_kbps.store(kbps, Relaxed);
            (win_start, win_bytes) = (Instant::now(), 0);
        }
        chunks.clear();
        ingest.feed(bytes, &mut chunks);
        let a = &ingest.analyzer;
        pipe.cc_errors.store(a.cc_errors, Relaxed);
        pipe.pts_discontinuities
            .store(a.pts_discontinuities, Relaxed);
        pipe.sync_losses.store(ingest.sync_losses, Relaxed);
        pipe.keyframes.store(a.keyframes, Relaxed);
        if ingest.analyzer.take_psi_changed() {
            if let Some((pat, pmt)) = ingest.analyzer.psi() {
                pipe.set_psi(pat, pmt);
            }
        }
        if let Some(info) = ingest.analyzer.take_info_changed() {
            pipe.info_tx.send_replace(Some(info));
        }
        for c in chunks.drain(..) {
            if c.key && pipe.ttk_ms.load(Relaxed) == UNSET {
                pipe.ttk_ms
                    .store(pipe.created.elapsed().as_millis() as u64, Relaxed);
            }
            pipe.publish(c, inner.cfg.gop_max_bytes);
        }
    }
}

async fn upstream_loop(inner: Arc<Inner>, pipe: Arc<Pipe>, spec: ChannelSpec) {
    let mut ingest = Ingest::new();
    let (mut idx, mut fails, mut attempt) = (0usize, 0u32, 0u32);
    loop {
        if pipe.cancel.is_cancelled() {
            return;
        }
        let cand = &spec.candidates[idx];
        match run_connection(&inner, &pipe, cand, &mut ingest).await {
            ConnEnd::Cancelled => return,
            ConnEnd::Failed { err, flowed } => {
                let err = crate::net::redact_text(&err);
                // Providers with a connection limit free slots with a lag after a disconnect:
                // retry a fresh channel quickly for a short window, then give up with a clear error
                // instead of spinning.
                let never_live = pipe.ttfb_ms.load(Relaxed) == UNSET;
                if never_live && pipe.max_conn > 0 && is_busy_err(&err) {
                    *pipe.last_error.lock().unwrap() = Some(err.clone());
                    if pipe.created.elapsed() < inner.cfg.busy_retry_window {
                        debug!(channel = pipe.id, "provider busy ({err}); retrying shortly");
                        tokio::select! {
                            _ = pipe.cancel.cancelled() => return,
                            _ = tokio::time::sleep(inner.cfg.busy_retry_step) => {}
                        }
                        continue;
                    }
                    if spec.candidates.len() == 1 {
                        let msg = format!(
                            "the provider refused the connection ({err}); its limit of {} \
                             concurrent connection(s) is probably still in use. Try again in a few seconds",
                            pipe.max_conn
                        );
                        warn!(channel = pipe.id, name = %pipe.name, "{msg}");
                        *pipe.fatal.lock().unwrap() = Some(msg);
                        pipe.set_state(PipeState::Reconnecting);
                        {
                            let mut st = inner.state.lock().unwrap();
                            if st
                                .pipes
                                .get(&pipe.id)
                                .is_some_and(|p| Arc::ptr_eq(p, &pipe))
                            {
                                st.pipes.remove(&pipe.id);
                            }
                        }
                        pipe.cancel.cancel();
                        return;
                    }
                }
                warn!(channel = pipe.id, name = %pipe.name, url_host = ?host_of(&cand.url), "upstream failed: {err}");
                *pipe.last_error.lock().unwrap() = Some(err);
                pipe.reconnects.fetch_add(1, Relaxed);
                pipe.set_state(PipeState::Reconnecting);
                pipe.clear_gop();
                if flowed >= inner.cfg.healthy_after {
                    fails = 0;
                    attempt = 0;
                }
                fails += 1;
                attempt += 1;
                if fails >= inner.cfg.failover_after && spec.candidates.len() > 1 {
                    // try the following candidates until one has a free provider slot
                    for step in 1..=spec.candidates.len() {
                        let next = (idx + step) % spec.candidates.len();
                        let c = &spec.candidates[next];
                        let ok = {
                            let mut st = inner.state.lock().unwrap();
                            if c.source_id == pipe.source() {
                                true
                            } else {
                                ChannelHub::ensure_slot(
                                    &mut st,
                                    c.source_id,
                                    c.max_connections,
                                    Some(pipe.id),
                                )
                                .is_ok()
                            }
                        };
                        if ok {
                            if next != idx {
                                info!(
                                    channel = pipe.id,
                                    from = idx,
                                    to = next,
                                    "failing over to alias"
                                );
                                pipe.source_id.store(c.source_id, Relaxed);
                                pipe.active_candidate.store(next, Relaxed);
                                pipe.failovers.fetch_add(1, Relaxed);
                                idx = next;
                            }
                            fails = 0;
                            break;
                        }
                    }
                }
                let wait = jitter(inner.cfg.backoff_base, attempt - 1, inner.cfg.backoff_max);
                tokio::select! {
                    _ = pipe.cancel.cancelled() => return,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(pid: u16, pusi: bool, cc: u8, fill: u8) -> Vec<u8> {
        let mut p = vec![fill; PKT];
        p[0] = SYNC;
        p[1] = (if pusi { 0x40 } else { 0 }) | (pid >> 8) as u8;
        p[2] = pid as u8;
        p[3] = 0x10 | (cc & 0x0F);
        p
    }

    #[test]
    fn ingest_aligns_arbitrary_chunking_and_recovers_sync() {
        let mut stream = Vec::new();
        for i in 0..40u8 {
            stream.extend(pkt(0x100, false, i, 0xAA));
        }
        // garbage in the middle forces a resync
        let mut dirty = stream[..PKT * 10].to_vec();
        dirty.extend_from_slice(&[1, 2, 3, 4, 5]);
        dirty.extend_from_slice(&stream[PKT * 10..]);

        let mut ing = Ingest::new();
        let mut out = vec![];
        for piece in dirty.chunks(333) {
            ing.feed(Bytes::copy_from_slice(piece), &mut out);
        }
        let total: usize = out.iter().map(|c| c.bytes.len()).sum();
        assert_eq!(total % PKT, 0);
        assert!(
            out.iter()
                .all(|c| c.bytes.len() % PKT == 0 && c.bytes[0] == SYNC)
        );
        assert_eq!(total, 40 * PKT, "no packet lost or duplicated");
        assert!(ing.sync_losses >= 1);
    }

    #[test]
    fn gop_cache_replaces_on_keyframe_and_late_subscriber_gets_psi_first() {
        let spec = ChannelSpec {
            channel_id: 1,
            name: "t".into(),
            candidates: vec![],
        };
        let pipe = Pipe::new(&spec, &HubConfig::default(), 1, Duration::from_secs(1));
        pipe.set_psi(
            Bytes::from(pkt(0, true, 0, 1)),
            Bytes::from(pkt(0x100, true, 0, 2)),
        );
        let mk = |key: bool, fill: u8| Chunk {
            bytes: Bytes::from(pkt(0x101, key, 0, fill)),
            key,
        };
        // before any keyframe: nothing cached → subscriber must wait for sync
        pipe.publish(mk(false, 9), 1 << 20);
        let (init, _rx, need) = pipe.snapshot();
        assert!(init.is_empty() && need);
        pipe.publish(mk(true, 1), 1 << 20);
        pipe.publish(mk(false, 2), 1 << 20);
        pipe.publish(mk(true, 3), 1 << 20); // newer GOP replaces the older
        pipe.publish(mk(false, 4), 1 << 20);
        let (init, _rx, need) = pipe.snapshot();
        assert!(!need);
        let fills: Vec<u8> = init.iter().map(|b| b[10]).collect();
        assert_eq!(fills, vec![1, 2, 3, 4], "PAT, PMT, then newest GOP only");
        // cap exceeded → cache dropped until next keyframe
        pipe.publish(mk(false, 5), 2 * PKT);
        assert!(pipe.snapshot().2);
    }
}

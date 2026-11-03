//! Transcode / remux session manager.

use crate::decision::{
    AudioAction, PlaybackDecision, PlaybackMode, SubtitleAction, VideoAction, is_image_subtitle,
    normalize_codec,
};
use crate::error::PlaybackError;
use crate::ffmpeg::{self, BuildParams, BurnIn, INIT_NAME};
use crate::fmp4;
use crate::hwaccel::HwAccel;
use crate::hwaccel::HwAccelStatus;
use crate::model::{FileInfo, Hdr, StreamInfo, StreamKind};
use crate::plan::{self, SegmentPlan};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use utoipa::ToSchema;

/// The only HLS variant name served (`/stream/{session}/main/index.m3u8`).
pub const VARIANT: &str = "main";

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub ffmpeg_path: String,
    /// Sessions write into `data_dir/transcode/{session}/`.
    pub data_dir: PathBuf,
    pub segment_secs: u32,
    /// ffmpeg-backed sessions idle (no segment request / progress ping) this long are killed.
    pub idle_timeout: Duration,
    /// Direct-play sessions without a progress ping this long are dropped.
    pub direct_idle_timeout: Duration,
    pub reaper_interval: Duration,
    /// Max concurrent (audio-)transcode sessions overall; 0 = unlimited.
    pub max_global_transcodes: usize,
    /// Max concurrent (audio-)transcode sessions per user; 0 = unlimited.
    pub max_user_transcodes: usize,
    /// How long a segment request waits for ffmpeg to produce the file.
    pub segment_wait_timeout: Duration,
    /// A request at most this many segments beyond ffmpeg's head waits instead of restarting.
    pub lookahead_segments: u32,
    /// ffprobe binary (keyframe probing for copy-mode playlists).
    pub ffprobe_path: String,
    pub keyframe_probe_timeout: Duration,
    /// Budget for reading the keyframe index from container metadata during session creation
    /// (`decide` never waits longer than this for indexing).
    pub keyframe_fast_timeout: Duration,
    /// Time limit of the low-priority background ffprobe scan used when no fast index exists.
    pub background_probe_timeout: Duration,
    /// ffmpeg is paused (SIGSTOP) once it is more than this many segments ahead of the last
    /// requested segment, and resumed when the client catches up. 0 disables throttling.
    pub max_ahead_segments: u32,
    pub throttle_interval: Duration,
    /// A HW-backed ffmpeg failing within this window before producing output triggers the
    /// software fallback.
    pub hw_startup_grace: Duration,
}

impl SessionConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        SessionConfig {
            ffmpeg_path: "ffmpeg".into(),
            data_dir: data_dir.into(),
            segment_secs: 4,
            idle_timeout: Duration::from_secs(60),
            direct_idle_timeout: Duration::from_secs(300),
            reaper_interval: Duration::from_secs(5),
            max_global_transcodes: 0,
            max_user_transcodes: 0,
            segment_wait_timeout: Duration::from_secs(20),
            lookahead_segments: 3,
            ffprobe_path: "ffprobe".into(),
            keyframe_probe_timeout: Duration::from_secs(300),
            keyframe_fast_timeout: Duration::from_secs(2),
            background_probe_timeout: Duration::from_secs(3 * 3600),
            max_ahead_segments: 20,
            throttle_interval: Duration::from_millis(500),
            hw_startup_grace: Duration::from_secs(15),
        }
    }
    pub fn transcode_root(&self) -> PathBuf {
        self.data_dir.join("transcode")
    }
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub user_id: i64,
    pub device_id: Option<String>,
    pub file: FileInfo,
    pub streams: Vec<StreamInfo>,
    pub decision: PlaybackDecision,
    pub start_position_s: f64,
}

/// Snapshot of a session for the admin dashboard / `playback_sessions` mirroring.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionInfo {
    pub id: String,
    pub user_id: i64,
    pub device_id: Option<String>,
    pub file_id: i64,
    pub mode: PlaybackMode,
    pub decision: PlaybackDecision,
    pub started_at: i64,
    pub last_ping: i64,
    pub position_s: f64,
    pub bitrate_kbps: Option<u32>,
    pub hwaccel: String,
    /// ffmpeg currently running for this session.
    pub active: bool,
    /// ffmpeg is paused by the read-ahead throttle.
    #[serde(default)]
    pub paused: bool,
    /// No keyframe index was available: ffmpeg's own growing EVENT playlist is served and seeking
    /// is approximate until the background index completes (next session is exact).
    #[serde(default)]
    pub fallback_playlist: bool,
}

struct Meta {
    last_ping: i64,
    last_activity: Instant,
    position_s: f64,
    bitrate_kbps: Option<u32>,
}

struct Proc {
    child: Option<Child>,
    start_segment: u32,
    paused: bool,
    started: Instant,
}

struct Session {
    id: String,
    user_id: i64,
    device_id: Option<String>,
    file: FileInfo,
    decision: PlaybackDecision,
    dir: Option<PathBuf>,
    started_at: i64,
    plan: SegmentPlan,
    event_mode: bool,
    total_segments: u32,
    hw: Mutex<HwAccel>,
    last_req: AtomicU32,
    source_video_codec: Option<String>,
    source_hdr: Hdr,
    burn_in: Option<BurnIn>,
    meta: Mutex<Meta>,
    proc: tokio::sync::Mutex<Proc>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct SessionManager {
    cfg: SessionConfig,
    hw: HwAccel,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    degraded: Mutex<Vec<String>>,
    indexer: Arc<Indexer>,
}

impl SessionManager {
    /// Create a manager. Removes stale transcode directories left by a previous run.
    pub fn new(cfg: SessionConfig, hw: HwAccel) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(cfg.transcode_root());
        Arc::new(SessionManager {
            cfg,
            hw,
            sessions: Mutex::new(HashMap::new()),
            degraded: Mutex::new(Vec::new()),
            indexer: Arc::new(Indexer::default()),
        })
    }

    pub fn config(&self) -> &SessionConfig {
        &self.cfg
    }

    /// The configured (detected) backend.
    pub fn hwaccel(&self) -> &HwAccel {
        &self.hw
    }

    /// Backend new sessions will use: software once the configured one has been marked degraded.
    pub fn effective_hwaccel(&self) -> HwAccel {
        if self
            .degraded
            .lock()
            .unwrap()
            .iter()
            .any(|d| d == self.hw.name())
        {
            HwAccel::Software
        } else {
            self.hw.clone()
        }
    }

    /// Names of backends that failed at runtime and are no longer used.
    pub fn degraded_backends(&self) -> Vec<String> {
        self.degraded.lock().unwrap().clone()
    }

    /// Detection result overlaid with runtime degradation (for the admin hw-accel endpoint).
    pub fn hwaccel_status(&self, detected: &HwAccelStatus) -> HwAccelStatus {
        let mut st = detected.clone();
        st.degraded = self.degraded_backends();
        if st.degraded.iter().any(|d| d == st.selected.name()) {
            st.selected = HwAccel::Software;
        }
        st
    }

    /// Spawn the idle reaper; it stops when the manager is dropped.
    pub fn spawn_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak: Weak<SessionManager> = Arc::downgrade(self);
        let interval = self.cfg.reaper_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(m) = weak.upgrade() else { break };
                m.reap_idle().await;
            }
        })
    }

    /// Create a session. ffmpeg-backed sessions start producing immediately at `start_position_s`.
    pub async fn create_session(&self, req: StartRequest) -> Result<SessionInfo, PlaybackError> {
        let mode = req.decision.mode;
        let counts = matches!(mode, PlaybackMode::Transcode | PlaybackMode::AudioTranscode);
        let id = uuid::Uuid::new_v4().to_string();
        let seg = self.cfg.segment_secs;
        let duration = req.file.duration_s.unwrap_or(0.0);
        if mode.needs_ffmpeg() && duration <= 0.0 {
            return Err(PlaybackError::Invalid("file duration unknown".into()));
        }
        let mut plan = SegmentPlan::fixed(if mode.needs_ffmpeg() { duration } else { 0.0 }, seg);
        let mut event_mode = false;
        let remote = crate::ffmpeg::is_remote(&req.file.path);
        if remote && mode.needs_ffmpeg() && req.decision.video == VideoAction::Copy {
            // Remote (provider) input: never scan it for keyframes — that would download the whole
            // file over the provider connection. ffmpeg's own growing EVENT playlist is served.
            event_mode = true;
        } else if mode.needs_ffmpeg() && req.decision.video == VideoAction::Copy {
            // 1. disk cache, 2. container metadata (bounded, ms), 3. fall back to an EVENT
            // playlist while a background scan builds the cache. Never blocks on a full scan.
            let (dd, fid, path) = (&self.cfg.data_dir, req.file.id, req.file.path.as_str());
            let kfs = match plan::cached_keyframes(dd, fid, path).await {
                Some(k) => Some(k),
                None => {
                    let k =
                        plan::fast_keyframes(path, self.cfg.keyframe_fast_timeout, duration).await;
                    if let Some(k) = &k {
                        plan::store_keyframes(dd, fid, path, k).await;
                    }
                    k
                }
            };
            match kfs {
                Some(k) if !k.is_empty() => plan = SegmentPlan::from_keyframes(&k, duration, seg),
                _ => {
                    tracing::warn!(
                        file = fid,
                        "no fast keyframe index; serving EVENT playlist while indexing in background"
                    );
                    event_mode = true;
                }
            }
        }
        let total_segments = plan.len();

        let video = req
            .streams
            .iter()
            .find(|s| Some(s.idx) == req.decision.video_index && s.kind == StreamKind::Video);
        let burn_in = if req.decision.subtitle == SubtitleAction::BurnIn {
            req.decision.subtitle_index.and_then(|si| {
                let s = req
                    .streams
                    .iter()
                    .find(|s| s.kind == StreamKind::Subtitle && s.idx == si)?;
                let ordinal = req
                    .streams
                    .iter()
                    .filter(|x| {
                        x.kind == StreamKind::Subtitle && x.external_path.is_none() && x.idx < s.idx
                    })
                    .count() as u32;
                Some(BurnIn {
                    ordinal,
                    image: is_image_subtitle(s.codec.as_deref().unwrap_or("")),
                    external_path: s.external_path.clone(),
                })
            })
        } else {
            None
        };

        let dir = mode
            .needs_ffmpeg()
            .then(|| self.cfg.transcode_root().join(&id));
        let session = Arc::new(Session {
            id: id.clone(),
            user_id: req.user_id,
            device_id: req.device_id,
            file: req.file,
            decision: req.decision,
            dir: dir.clone(),
            started_at: now_unix(),
            plan,
            event_mode,
            total_segments,
            hw: Mutex::new(self.effective_hwaccel()),
            last_req: AtomicU32::new(0),
            source_video_codec: video.and_then(|v| v.codec.as_deref().map(normalize_codec)),
            source_hdr: video.map(|v| v.hdr).unwrap_or_default(),
            burn_in,
            meta: Mutex::new(Meta {
                last_ping: now_unix(),
                last_activity: Instant::now(),
                position_s: req.start_position_s,
                bitrate_kbps: None,
            }),
            proc: tokio::sync::Mutex::new(Proc {
                child: None,
                start_segment: 0,
                paused: false,
                started: Instant::now(),
            }),
        });

        {
            let mut map = self.sessions.lock().unwrap();
            if counts {
                let live = |s: &&Arc<Session>| {
                    matches!(
                        s.decision.mode,
                        PlaybackMode::Transcode | PlaybackMode::AudioTranscode
                    )
                };
                let global = map.values().filter(live).count();
                if self.cfg.max_global_transcodes > 0 && global >= self.cfg.max_global_transcodes {
                    return Err(PlaybackError::GlobalLimit(self.cfg.max_global_transcodes));
                }
                let user = map
                    .values()
                    .filter(live)
                    .filter(|s| s.user_id == req.user_id)
                    .count();
                if self.cfg.max_user_transcodes > 0 && user >= self.cfg.max_user_transcodes {
                    return Err(PlaybackError::UserLimit(self.cfg.max_user_transcodes));
                }
            }
            map.insert(id.clone(), session.clone());
        }
        if event_mode && !remote {
            self.indexer
                .acquire(&self.cfg, session.file.id, &session.file.path);
        }

        if let Some(dir) = &dir {
            // map the resume position through the plan (real keyframe boundaries in copy mode)
            let start = session
                .plan
                .segment_at(req.start_position_s.max(0.0))
                .min(total_segments.saturating_sub(1));
            session.last_req.store(start, Ordering::Relaxed);
            let res = async {
                tokio::fs::create_dir_all(dir).await?;
                let mut p = session.proc.lock().await;
                self.start_ffmpeg(&session, &mut p, start).await
            }
            .await;
            if let Err(e) = res {
                self.remove(&id).await;
                return Err(e);
            }
        }
        if dir.is_some() && self.cfg.max_ahead_segments > 0 {
            self.spawn_throttle(Arc::downgrade(&session));
        }
        Ok(self.info_of(&session).await)
    }

    /// Per-session task pausing ffmpeg when it runs too far ahead of the client.
    fn spawn_throttle(&self, weak: Weak<Session>) {
        let max_ahead = self.cfg.max_ahead_segments;
        let resume_at = max_ahead - (max_ahead / 4).max(1).min(max_ahead);
        let interval = self.cfg.throttle_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(s) = weak.upgrade() else { break };
                let Some(dir) = s.dir.clone() else { break };
                let mut p = s.proc.lock().await;
                let alive = p
                    .child
                    .as_mut()
                    .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
                if !alive {
                    continue;
                }
                let head = s.head(&dir, p.start_segment);
                let ahead =
                    head.saturating_sub(s.last_req.load(Ordering::Relaxed).max(p.start_segment));
                if !p.paused && ahead > max_ahead {
                    if let Some(c) = p.child.as_ref() {
                        tracing::debug!(session = %s.id, ahead, "pausing ffmpeg (read-ahead limit)");
                        signal(c, Sig::Stop);
                        p.paused = true;
                    }
                } else if p.paused && ahead <= resume_at {
                    if let Some(c) = p.child.as_ref() {
                        signal(c, Sig::Cont);
                    }
                    p.paused = false;
                }
            }
        });
    }

    async fn info_of(&self, s: &Session) -> SessionInfo {
        let (active, paused) = match s.proc.try_lock() {
            Ok(mut p) => (
                p.child
                    .as_mut()
                    .is_some_and(|c| matches!(c.try_wait(), Ok(None))),
                p.paused,
            ),
            Err(_) => (true, false),
        };
        let m = s.meta.lock().unwrap();
        SessionInfo {
            id: s.id.clone(),
            user_id: s.user_id,
            device_id: s.device_id.clone(),
            file_id: s.file.id,
            mode: s.decision.mode,
            decision: s.decision.clone(),
            started_at: s.started_at,
            last_ping: m.last_ping,
            position_s: m.position_s,
            bitrate_kbps: m.bitrate_kbps.or_else(|| match &s.decision.video {
                VideoAction::Encode { bitrate_kbps, .. } => Some(*bitrate_kbps),
                _ => s.file.bitrate.map(|b| (b / 1000) as u32),
            }),
            hwaccel: s.hw.lock().unwrap().name().into(),
            active,
            paused,
            fallback_playlist: s.event_mode,
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Session>, PlaybackError> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(PlaybackError::SessionNotFound)
    }

    pub async fn session(&self, id: &str) -> Result<SessionInfo, PlaybackError> {
        let s = self.get(id)?;
        Ok(self.info_of(&s).await)
    }

    /// All sessions (for the admin dashboard).
    pub async fn list(&self) -> Vec<SessionInfo> {
        let all: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().cloned().collect();
        let mut out = Vec::with_capacity(all.len());
        for s in all {
            out.push(self.info_of(&s).await);
        }
        out.sort_by_key(|i| i.started_at);
        out
    }

    /// Progress ping from the client; also keeps the session alive.
    /// Keep a session alive: any request for its media (segments, playlists, proxied bytes) from
    /// any client counts, e.g. a Chromecast fetching while the sender only pings progress.
    pub fn touch_session(&self, id: &str) {
        if let Ok(s) = self.get(id) {
            s.meta.lock().unwrap().last_activity = Instant::now();
        }
    }

    pub fn update_progress(
        &self,
        id: &str,
        position_s: f64,
        bitrate_kbps: Option<u32>,
    ) -> Result<(), PlaybackError> {
        let s = self.get(id)?;
        let mut m = s.meta.lock().unwrap();
        m.last_ping = now_unix();
        m.last_activity = Instant::now();
        m.position_s = position_s;
        if bitrate_kbps.is_some() {
            m.bitrate_kbps = bitrate_kbps;
        }
        Ok(())
    }

    /// Kill the session's ffmpeg and delete its directory. Returns false if unknown.
    pub async fn remove(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id);
        let Some(s) = removed else { return false };
        {
            let mut p = s.proc.lock().await;
            if let Some(mut c) = p.child.take() {
                let _ = c.kill().await;
            }
        }
        if let Some(dir) = &s.dir {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        if s.event_mode && !crate::ffmpeg::is_remote(&s.file.path) {
            self.indexer.release(s.file.id);
        }
        true
    }

    /// Number of background keyframe scans currently queued or running.
    pub fn background_index_jobs(&self) -> usize {
        self.indexer.jobs()
    }

    /// Remove every session (graceful shutdown).
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.remove(&id).await;
        }
        self.indexer.abort_all();
    }

    /// Kill sessions idle beyond their timeout; returns the reaped ids.
    pub async fn reap_idle(&self) -> Vec<String> {
        let victims: Vec<String> = {
            let map = self.sessions.lock().unwrap();
            map.values()
                .filter(|s| {
                    let limit = if s.decision.mode.needs_ffmpeg() {
                        self.cfg.idle_timeout
                    } else {
                        self.cfg.direct_idle_timeout
                    };
                    s.meta.lock().unwrap().last_activity.elapsed() >= limit
                })
                .map(|s| s.id.clone())
                .collect()
        };
        for id in &victims {
            tracing::info!(session = %id, "reaping idle playback session");
            self.remove(id).await;
        }
        victims
    }

    /// Directory of a session (None for direct play).
    pub fn session_dir(&self, id: &str) -> Result<Option<PathBuf>, PlaybackError> {
        Ok(self.get(id)?.dir.clone())
    }

    // ---------------- playlists ----------------

    /// `master.m3u8` for the session.
    pub fn master_playlist(&self, id: &str) -> Result<String, PlaybackError> {
        let s = self.get(id)?;
        self.touch(&s);
        if s.dir.is_none() {
            return Err(PlaybackError::Invalid(
                "direct-play sessions have no HLS playlist".into(),
            ));
        }
        let (w, h, vb) = match &s.decision.video {
            VideoAction::Encode {
                width,
                height,
                bitrate_kbps,
                ..
            } => (Some(*width), Some(*height), *bitrate_kbps),
            _ => (
                None,
                None,
                s.file.bitrate.map(|b| (b / 1000) as u32).unwrap_or(8000),
            ),
        };
        let ab = match &s.decision.audio {
            AudioAction::Encode { bitrate_kbps, .. } => *bitrate_kbps,
            _ => 192,
        };
        Ok(build_master_playlist(((vb + ab) as u64) * 1000, w.zip(h)))
    }

    /// Full VOD media playlist (`main/index.m3u8`), generated up front from the duration.
    ///
    /// For sessions without a keyframe index (`fallback_playlist`) this returns ffmpeg's growing
    /// EVENT playlist as written so far (error if nothing is produced yet); prefer
    /// [`SessionManager::media_playlist_async`], which waits for the first segment.
    pub fn media_playlist(&self, id: &str) -> Result<String, PlaybackError> {
        let s = self.get(id)?;
        self.touch(&s);
        if s.event_mode {
            let dir = s.dir.as_ref().ok_or(PlaybackError::SessionNotFound)?;
            return std::fs::read_to_string(dir.join(ffmpeg::FFMPEG_PLAYLIST))
                .ok()
                .and_then(|t| event_playlist(&t))
                .ok_or_else(|| PlaybackError::SegmentUnavailable("playlist not ready".into()));
        }
        Ok(build_plan_playlist(&s.plan, self.cfg.segment_secs))
    }

    /// Like [`SessionManager::media_playlist`] but waits (up to `segment_wait_timeout`) for
    /// ffmpeg's first segment in fallback (EVENT playlist) mode.
    pub async fn media_playlist_async(&self, id: &str) -> Result<String, PlaybackError> {
        let s = self.get(id)?;
        if !s.event_mode {
            return self.media_playlist(id);
        }
        let dir = s.dir.clone().ok_or(PlaybackError::SessionNotFound)?;
        let deadline = Instant::now() + self.cfg.segment_wait_timeout;
        loop {
            if !self.sessions.lock().unwrap().contains_key(id) {
                return Err(PlaybackError::SessionNotFound);
            }
            self.touch(&s);
            if let Some(t) = tokio::fs::read_to_string(dir.join(ffmpeg::FFMPEG_PLAYLIST))
                .await
                .ok()
                .and_then(|t| event_playlist(&t))
            {
                return Ok(t);
            }
            let failed = {
                let mut p = s.proc.lock().await;
                matches!(p.child.as_mut().map(|c| c.try_wait()), Some(Ok(Some(st))) if !st.success())
            };
            if failed || Instant::now() >= deadline {
                return Err(PlaybackError::SegmentUnavailable(format!(
                    "no playlist from ffmpeg: {}",
                    read_log_tail(&dir).await
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn touch(&self, s: &Session) {
        s.meta.lock().unwrap().last_activity = Instant::now();
    }

    // ---------------- segments ----------------

    /// Resolve `init.mp4` / `seg_NNNNN.m4s` to an existing file, (re)starting ffmpeg as needed.
    pub async fn segment_path(&self, id: &str, name: &str) -> Result<PathBuf, PlaybackError> {
        let s = self.get(id)?;
        self.touch(&s);
        let dir = s
            .dir
            .clone()
            .ok_or_else(|| PlaybackError::Invalid("no HLS output for this session".into()))?;
        let wanted: Option<u32> = if name == INIT_NAME {
            None
        } else {
            let n = ffmpeg::parse_segment_name(name)
                .ok_or_else(|| PlaybackError::Invalid(format!("bad segment name {name}")))?;
            if n >= s.total_segments {
                return Err(PlaybackError::Invalid("segment beyond end of media".into()));
            }
            s.last_req.store(n, Ordering::Relaxed);
            Some(n)
        };
        let path = dir.join(name);
        let deadline = Instant::now() + self.cfg.segment_wait_timeout;
        let mut restarts = 0;
        loop {
            // a removed/reaped session must not respawn ffmpeg or recreate its directory
            if !self.sessions.lock().unwrap().contains_key(id) {
                return Err(PlaybackError::SessionNotFound);
            }
            self.touch(&s);
            if wanted.is_none() {
                if let Some(ready) = ready_init(&dir).await {
                    self.touch(&s);
                    return Ok(ready);
                }
            } else if path.exists() {
                // segments are written via temp file + rename (or assembled likewise): complete
                if let (Some(n), false, true) = (
                    wanted,
                    s.event_mode,
                    matches!(s.decision.video, VideoAction::Encode { .. }),
                ) {
                    // encoded fixed grid: snap the run-dependent timeline shift to n * segment_secs
                    normalize_encoded(&dir, &path, s.plan.start(n)).await;
                }
                self.touch(&s);
                return Ok(path);
            }
            if let (Some(n), true) = (wanted, s.plan.is_keyframe_plan()) {
                if assemble_segment(&dir, &s.plan, n).await? {
                    return Ok(path);
                }
            }
            {
                let mut p = s.proc.lock().await;
                let state = p.child.as_mut().map(|c| c.try_wait());
                let alive = matches!(state, Some(Ok(None)));
                let failed = matches!(state, Some(Ok(Some(st))) if !st.success());
                let target = wanted.unwrap_or(p.start_segment);
                let mut fallback = false;
                if failed {
                    let cur = s.hw.lock().unwrap().clone();
                    let encoding = matches!(s.decision.video, VideoAction::Encode { .. });
                    if encoding
                        && cur != HwAccel::Software
                        && p.started.elapsed() < self.cfg.hw_startup_grace
                        && !s.have_started(&dir, p.start_segment)
                    {
                        tracing::warn!(
                            session = %id,
                            backend = cur.name(),
                            "hardware ffmpeg failed before producing output; retrying with software encoding: {}",
                            read_log_tail(&dir).await
                        );
                        {
                            let mut d = self.degraded.lock().unwrap();
                            if !d.iter().any(|x| x == cur.name()) {
                                d.push(cur.name().to_string());
                            }
                        }
                        *s.hw.lock().unwrap() = HwAccel::Software;
                        fallback = true;
                    }
                }
                if p.paused && alive {
                    // the client needs something ffmpeg has not produced yet
                    if let Some(c) = p.child.as_ref() {
                        signal(c, Sig::Cont);
                    }
                    p.paused = false;
                }
                let must_restart = if fallback {
                    true
                } else if alive {
                    match wanted {
                        None => false,
                        Some(n) => {
                            let head = s.head(&dir, p.start_segment);
                            n < p.start_segment || n > head + self.cfg.lookahead_segments
                        }
                    }
                } else {
                    true
                };
                if must_restart {
                    if !fallback {
                        if restarts >= 2 {
                            return Err(PlaybackError::SegmentUnavailable(format!(
                                "ffmpeg failed: {}",
                                read_log_tail(&dir).await
                            )));
                        }
                        restarts += 1;
                    }
                    tracing::info!(session = %id, segment = target, "restarting ffmpeg");
                    self.start_ffmpeg(&s, &mut p, target).await?;
                }
            }
            if Instant::now() >= deadline {
                return Err(PlaybackError::SegmentUnavailable(format!(
                    "timeout waiting for {name}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn start_ffmpeg(
        &self,
        s: &Session,
        p: &mut Proc,
        start_segment: u32,
    ) -> Result<(), PlaybackError> {
        if let Some(mut c) = p.child.take() {
            let _ = c.kill().await;
        }
        p.paused = false;
        let dir = s
            .dir
            .as_ref()
            .ok_or_else(|| PlaybackError::Invalid("no output dir".into()))?;
        tokio::fs::create_dir_all(dir).await?;
        let hw = s.hw.lock().unwrap().clone();
        let params = BuildParams {
            input: &s.file.path,
            decision: &s.decision,
            hwaccel: &hw,
            source_video_codec: s.source_video_codec.as_deref(),
            source_hdr: s.source_hdr,
            burn_in: s.burn_in.clone(),
            start_segment,
            segment_secs: self.cfg.segment_secs,
            out_dir: dir,
        };
        let args = if s.plan.is_keyframe_plan() {
            ffmpeg::build_args_with_plan(&params, &s.plan)
        } else {
            ffmpeg::build_args(&params)
        };
        let log = std::fs::File::create(dir.join("ffmpeg.log"))?;
        let child = Command::new(&self.cfg.ffmpeg_path)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PlaybackError::Ffmpeg(format!("spawn {}: {e}", self.cfg.ffmpeg_path)))?;
        p.child = Some(child);
        p.start_segment = start_segment;
        p.started = Instant::now();
        Ok(())
    }
}

/// ffmpeg's playlist, once it lists a segment, with `#EXT-X-START` so players begin at 0
/// instead of the live edge.
fn event_playlist(text: &str) -> Option<String> {
    if !text.contains("#EXTINF") {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 32);
    for (i, line) in text.lines().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i == 0 {
            out.push_str("#EXT-X-START:TIME-OFFSET=0\n");
        }
    }
    Some(out)
}

struct ProbeJob {
    users: usize,
    handle: tokio::task::JoinHandle<()>,
}

/// Deduplicated, globally serialised, low-priority background ffprobe keyframe scans.
struct Indexer {
    sem: Arc<tokio::sync::Semaphore>,
    jobs: Mutex<HashMap<i64, ProbeJob>>,
}

impl Default for Indexer {
    fn default() -> Self {
        Indexer {
            sem: Arc::new(tokio::sync::Semaphore::new(1)),
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

impl Indexer {
    fn acquire(self: &Arc<Self>, cfg: &SessionConfig, file_id: i64, path: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&file_id) {
            j.users += 1;
            return;
        }
        let (weak, sem) = (Arc::downgrade(self), self.sem.clone());
        let (ffprobe, data_dir, path, timeout) = (
            cfg.ffprobe_path.clone(),
            cfg.data_dir.clone(),
            path.to_string(),
            cfg.background_probe_timeout,
        );
        let handle = tokio::spawn(async move {
            // one scan at a time; dropping this future (abort) kills the ffprobe child
            let _permit = sem.acquire_owned().await;
            if plan::cached_keyframes(&data_dir, file_id, &path)
                .await
                .is_none()
            {
                match plan::probe_keyframes_cmd(&ffprobe, &path, timeout, true).await {
                    Ok(k) if !k.is_empty() => {
                        plan::store_keyframes(&data_dir, file_id, &path, &k).await;
                        tracing::info!(
                            file = file_id,
                            keyframes = k.len(),
                            "background keyframe index complete"
                        );
                    }
                    Ok(_) => tracing::warn!(
                        file = file_id,
                        "background keyframe scan found no keyframes"
                    ),
                    Err(e) => {
                        tracing::warn!(file = file_id, error = %e, "background keyframe scan failed")
                    }
                }
            }
            if let Some(ix) = weak.upgrade() {
                ix.jobs.lock().unwrap().remove(&file_id);
            }
        });
        jobs.insert(file_id, ProbeJob { users: 1, handle });
    }

    /// A session that needed the scan ended; kill the scan when nobody waits for it any more.
    fn release(&self, file_id: i64) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&file_id) {
            j.users = j.users.saturating_sub(1);
            if j.users == 0 {
                j.handle.abort();
                jobs.remove(&file_id);
            }
        }
    }

    fn jobs(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }

    fn abort_all(&self) {
        for (_, j) in self.jobs.lock().unwrap().drain() {
            j.handle.abort();
        }
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.abort_all();
    }
}

impl Session {
    /// A segment's output exists or is in progress.
    fn have_started(&self, dir: &Path, n: u32) -> bool {
        if dir.join(ffmpeg::segment_name(n)).exists() {
            return true;
        }
        self.plan.is_keyframe_plan() && dir.join(ffmpeg::gop_name(self.plan.first_gop(n))).exists()
    }

    /// First segment index >= `start` that has not been started yet.
    fn head(&self, dir: &Path, start: u32) -> u32 {
        let mut h = start;
        while h < self.total_segments && self.have_started(dir, h) {
            h += 1;
        }
        h
    }
}

/// True if `data` is a complete init segment: top-level boxes exactly cover the buffer, starting
/// with `ftyp` and including `moov`.
pub fn init_is_complete(data: &[u8]) -> bool {
    let mut pos = 0usize;
    let (mut first, mut moov) = (None, false);
    while pos + 8 <= data.len() {
        let size =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind = &data[pos + 4..pos + 8];
        if size < 8 || pos + size > data.len() {
            return false;
        }
        first.get_or_insert([kind[0], kind[1], kind[2], kind[3]]);
        moov |= kind == b"moov";
        pos += size;
    }
    pos == data.len() && first == Some(*b"ftyp") && moov
}

/// ffmpeg creates `init.mp4` before it is fully written and truncates it again on restarts, so
/// serve a validated copy (`init_ready.mp4`) that is created atomically.
async fn ready_init(dir: &Path) -> Option<PathBuf> {
    let ready = dir.join("init_ready.mp4");
    if ready.exists() {
        return Some(ready);
    }
    let data = tokio::fs::read(dir.join(INIT_NAME)).await.ok()?;
    if !init_is_complete(&data) {
        return None;
    }
    let tmp = dir.join("init_ready.mp4.tmp");
    tokio::fs::write(&tmp, &data).await.ok()?;
    tokio::fs::rename(&tmp, &ready).await.ok()?;
    Some(ready)
}

/// Rewrite an encoded segment in place so its first video sample sits exactly at `expected`
/// seconds (idempotent; leaves the file alone when already aligned or on any error).
async fn normalize_encoded(dir: &Path, path: &Path, expected: f64) {
    let Some(init) = ready_init(dir).await else {
        return;
    };
    let (Ok(init), Ok(mut data)) = (tokio::fs::read(init).await, tokio::fs::read(path).await)
    else {
        return;
    };
    let tracks = fmp4::init_tracks(&init);
    if matches!(fmp4::retime(&mut data, &tracks, expected), Some(d) if d != 0.0) {
        let tmp = path.with_extension("m4s.tmp");
        if tokio::fs::write(&tmp, &data).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, path).await;
        }
    }
}

/// Concatenate the GOP files of plan segment `n` into `seg_n.m4s` once all exist.
async fn assemble_segment(dir: &Path, plan: &SegmentPlan, n: u32) -> Result<bool, PlaybackError> {
    let (a, b) = plan.gop_range(n);
    for g in a..b {
        if !dir.join(ffmpeg::gop_name(g)).exists() {
            return Ok(false);
        }
    }
    // the validated init tells us each track's timescale for timeline normalisation
    let Some(init) = ready_init(dir).await else {
        return Ok(false);
    };
    let tracks = fmp4::init_tracks(&tokio::fs::read(init).await?);
    let mut out = Vec::new();
    for g in a..b {
        let mut gop = tokio::fs::read(dir.join(ffmpeg::gop_name(g))).await?;
        if let Some(t) = plan.gop_time(g) {
            // a run-dependent timeline shift (B-frame delay, audio priming) would make restarted
            // segments land off the playlist timeline; snap to the index time
            if fmp4::retime(&mut gop, &tracks, t).is_none() {
                tracing::warn!(
                    segment = n,
                    gop = g,
                    "could not normalise fragment timestamps"
                );
            }
        }
        out.extend(gop);
    }
    let tmp = dir.join(format!("{}.tmp", ffmpeg::segment_name(n)));
    tokio::fs::write(&tmp, out).await?;
    tokio::fs::rename(&tmp, dir.join(ffmpeg::segment_name(n))).await?;
    for g in a..b {
        let _ = tokio::fs::remove_file(dir.join(ffmpeg::gop_name(g))).await;
    }
    Ok(true)
}

#[derive(Clone, Copy)]
enum Sig {
    Stop,
    Cont,
}

#[cfg(unix)]
fn signal(child: &Child, sig: Sig) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    if let Some(pid) = child.id() {
        let s = match sig {
            Sig::Stop => Signal::SIGSTOP,
            Sig::Cont => Signal::SIGCONT,
        };
        let _ = kill(Pid::from_raw(pid as i32), s);
    }
}

#[cfg(not(unix))]
fn signal(_child: &Child, _sig: Sig) {}

async fn read_log_tail(dir: &Path) -> String {
    let log = tokio::fs::read_to_string(dir.join("ffmpeg.log"))
        .await
        .unwrap_or_default();
    log.lines()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn build_master_playlist(bandwidth_bps: u64, resolution: Option<(u32, u32)>) -> String {
    let res = resolution
        .map(|(w, h)| format!(",RESOLUTION={w}x{h}"))
        .unwrap_or_default();
    format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-STREAM-INF:BANDWIDTH={bandwidth_bps}{res}\n{VARIANT}/index.m3u8\n"
    )
}

/// VOD playlist from a segment plan (real keyframe boundaries in copy mode).
pub fn build_plan_playlist(plan: &SegmentPlan, min_target: u32) -> String {
    let target = (plan.max_seg_len().ceil() as u32)
        .max(min_target.min(1))
        .max(1);
    let mut o = String::new();
    o.push_str("#EXTM3U\n#EXT-X-VERSION:7\n");
    o.push_str(&format!("#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n"));
    o.push_str(&format!("#EXT-X-MAP:URI=\"{INIT_NAME}\"\n"));
    for n in 0..plan.len() {
        o.push_str(&format!(
            "#EXTINF:{:.3},\n{}\n",
            plan.seg_len(n),
            ffmpeg::segment_name(n)
        ));
    }
    o.push_str("#EXT-X-ENDLIST\n");
    o
}

pub fn build_vod_playlist(duration_s: f64, seg_secs: u32) -> String {
    let mut o = String::new();
    o.push_str("#EXTM3U\n#EXT-X-VERSION:7\n");
    o.push_str(&format!("#EXT-X-TARGETDURATION:{seg_secs}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n"));
    o.push_str(&format!("#EXT-X-MAP:URI=\"{INIT_NAME}\"\n"));
    let total = (duration_s / seg_secs as f64).ceil() as u32;
    for n in 0..total {
        let len = (duration_s - n as f64 * seg_secs as f64).min(seg_secs as f64);
        o.push_str(&format!("#EXTINF:{len:.3},\n{}\n", ffmpeg::segment_name(n)));
    }
    o.push_str("#EXT-X-ENDLIST\n");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_completeness_check() {
        let bx = |k: &[u8; 4], n: usize| {
            let mut v = ((n + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(k);
            v.extend(vec![0u8; n]);
            v
        };
        let full = [bx(b"ftyp", 8), bx(b"moov", 20)].concat();
        assert!(init_is_complete(&full));
        assert!(!init_is_complete(&[]));
        assert!(!init_is_complete(&full[..full.len() - 3]));
        assert!(!init_is_complete(&bx(b"ftyp", 8)));
    }

    #[test]
    fn vod_playlist_shape() {
        let p = build_vod_playlist(10.0, 4);
        assert!(p.contains("#EXT-X-MAP:URI=\"init.mp4\""));
        assert_eq!(p.matches("#EXTINF").count(), 3);
        assert!(p.contains("#EXTINF:4.000,\nseg_00001.m4s"));
        assert!(p.contains("#EXTINF:2.000,\nseg_00002.m4s"));
        assert!(p.trim_end().ends_with("#EXT-X-ENDLIST"));
    }

    #[test]
    fn master_playlist_shape() {
        let m = build_master_playlist(4_160_000, Some((1280, 720)));
        assert!(m.contains("BANDWIDTH=4160000,RESOLUTION=1280x720\nmain/index.m3u8"));
    }
}

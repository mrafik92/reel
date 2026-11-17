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
use crate::media::{MediaCtx, ProcPermit};
use crate::model::{FileInfo, Hdr, StreamInfo, StreamKind};
use crate::plan::{self, SegmentPlan};
use crate::redact::redact_text;
use crate::variants::VariantMeta;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use utoipa::ToSchema;

/// Name of a session's primary variant (`/stream/{session}/main/index.m3u8`). Its files live in the
/// session directory itself; further variants (rungs, #111) in `{session dir}/{variant id}/`.
pub const VARIANT: &str = "main";

/// An extra encoded variant of a session (a rung, #111). It shares the session's segment plan:
/// on a keyframe plan its IDR frames are forced at exactly the plan's boundaries, on a fixed
/// plan at the same absolute grid, so its segment `n` covers the same frames as every other
/// variant's. Its ffmpeg starts on the first request for one of its files.
#[derive(Debug, Clone)]
pub struct VariantSpec {
    /// URL path segment, `[a-z0-9]{1,16}`, unique within the session and not [`VARIANT`].
    pub id: String,
    /// Must encode the video; audio, subtitle and stream indexes should match the primary's.
    pub decision: PlaybackDecision,
    /// How the master playlist describes it (adaptive sessions).
    pub meta: Option<VariantMeta>,
}

/// A multi-variant session (#111): the primary variant (the session's decision) under its own
/// id plus encoded rungs, all listed in the master playlist. Each variant's ffmpeg runs only
/// while the player fetches it: it is admitted (transcode caps, process budget) when first
/// requested, counts against the caps until it is stopped, and is stopped after
/// `variant_idle_timeout` without requests.
#[derive(Debug, Clone)]
pub struct AdaptiveRequest {
    /// Id of the primary variant (`original`, or the top rung of a transcode-only session).
    pub primary_id: String,
    pub primary_meta: VariantMeta,
    pub rungs: Vec<VariantSpec>,
    /// The variant started right away (the player's start level).
    pub start: String,
}

/// State of one variant for the admin dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct VariantStatus {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    /// Average bitrate (kbit/s) the master playlist announces.
    pub bandwidth_kbps: u32,
    /// Video encoder, e.g. `hevc_vaapi (low power)`; `None` when the video is copied.
    pub encoder: Option<String>,
    /// `running`, `paused` (read-ahead throttle) or `idle` (no ffmpeg).
    pub state: String,
    /// Seconds since the player last requested one of its files.
    pub last_request_s: u64,
}

fn valid_variant_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

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
    /// ffmpeg-backed sessions whose last progress ping said `paused` get this long instead of
    /// `idle_timeout` (#95): a paused player with a full buffer requests nothing, and a background
    /// tab may throttle its pings. ffmpeg is already SIGSTOPped by the read-ahead throttle, so a
    /// paused session costs only its scratch directory and its transcode slot.
    pub paused_idle_timeout: Duration,
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
    /// Global budget of concurrent ffmpeg/ffprobe processes: every transcode *and remux* session,
    /// subtitle extraction and remote probe counts. 0 = default (6).
    pub max_ffmpeg_processes: usize,
    /// Disk budget of one session's scratch directory (older segments behind the playhead are
    /// dropped when exceeded).
    pub max_session_disk_bytes: u64,
    /// Disk budget of `data_dir/transcode` as a whole: idle sessions are evicted oldest-first,
    /// new transcodes are refused (503) while still over budget. 0 = unlimited.
    pub max_total_disk_bytes: u64,
    /// Sessions idle for at least this long may be evicted to free scratch space.
    pub disk_evict_min_idle: Duration,
    /// Adaptive sessions: a variant whose files nobody requested for this long has its ffmpeg
    /// stopped and its transcode slot / process slot released (#111).
    pub variant_idle_timeout: Duration,
    /// Lifetime of upstream-proxy tokens of a session (also revoked when the session ends).
    pub upstream_ttl: Duration,
    /// Bounds of reading a remote (provider) file's keyframe index from its container metadata.
    pub remote_index: plan::RemoteIndexLimits,
}

impl SessionConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        SessionConfig {
            ffmpeg_path: "ffmpeg".into(),
            data_dir: data_dir.into(),
            segment_secs: 4,
            idle_timeout: Duration::from_secs(60),
            direct_idle_timeout: Duration::from_secs(300),
            paused_idle_timeout: Duration::from_secs(30 * 60),
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
            max_ffmpeg_processes: crate::media::DEFAULT_MAX_FFMPEG_PROCESSES,
            max_session_disk_bytes: 4 << 30,
            max_total_disk_bytes: 20 << 30,
            disk_evict_min_idle: Duration::from_secs(15),
            variant_idle_timeout: Duration::from_secs(30),
            upstream_ttl: Duration::from_secs(12 * 3600),
            remote_index: plan::RemoteIndexLimits::default(),
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
    /// The user's concurrent-stream limit (every live session of the user counts). Enforced
    /// atomically inside `create_session`.
    pub user_max_streams: Option<usize>,
    /// The user's concurrent-transcode limit (enforced atomically like the stream limit).
    pub user_max_transcodes: Option<usize>,
    /// User-Agent for a remote (provider) input, e.g. the IPTV source's configured one. `None`:
    /// [`crate::ffmpeg::REMOTE_UA`]. Only the loopback proxy sends it; ffmpeg never sees it.
    pub user_agent: Option<String>,
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
    /// ffmpeg video encoder of a video transcode, e.g. `h264_vaapi (low power)`; `None` when the
    /// video is copied.
    #[serde(default)]
    pub encoder: Option<String>,
    /// ffmpeg currently running for this session.
    pub active: bool,
    /// ffmpeg is paused by the read-ahead throttle.
    #[serde(default)]
    pub paused: bool,
    /// The client's last progress ping said the player is paused (the session then gets
    /// `paused_idle_timeout` before it is reaped).
    #[serde(default)]
    pub client_paused: bool,
    /// No keyframe index was available: ffmpeg's own growing EVENT playlist is served. Players treat
    /// it as live and can't seek. Local files are indexed in the background (the next session is
    /// seekable); remote files only when their container has an index readable over `Range`.
    #[serde(default)]
    pub fallback_playlist: bool,
    /// Multi-variant sessions (#111): every variant with its state; empty otherwise.
    #[serde(default)]
    pub variants: Vec<VariantStatus>,
    /// The variant of the most recent segment request (what the player is being sent).
    #[serde(default)]
    pub delivering: Option<String>,
}

struct Meta {
    last_ping: i64,
    last_activity: Instant,
    /// The last progress ping said the player is paused.
    client_paused: bool,
    position_s: f64,
    bitrate_kbps: Option<u32>,
}

struct Proc {
    child: Option<Child>,
    start_segment: u32,
    paused: bool,
    started: Instant,
}

/// One rendition of a session, with its own ffmpeg (#111). All variants of a session share its
/// segment plan, input and burn-in file.
struct Variant {
    id: String,
    decision: PlaybackDecision,
    /// Output directory: the session directory for the primary variant, a subdirectory otherwise.
    dir: PathBuf,
    hw: Mutex<HwAccel>,
    last_req: AtomicU32,
    /// Incremented on every ffmpeg (re)start; in-flight post-processing of files from an older
    /// run is discarded.
    epoch: std::sync::atomic::AtomicU64,
    /// Timeline shift of the current ffmpeg run (plan time − actual time), measured on the first
    /// fragment processed after a (re)start and then applied to the whole run so continuity inside
    /// the run is preserved bit-exactly.
    run_delta: Mutex<Option<(u64, f64)>>,
    /// Redacted tail of ffmpeg's stderr (server-side diagnostics only).
    log: LogTail,
    proc: tokio::sync::Mutex<Proc>,
    meta: Option<VariantMeta>,
    /// Adaptive sessions: admitted (counts against the caps if it encodes, holds `permit`)
    /// from its first start until it is stopped for being idle.
    admitted: AtomicBool,
    permit: Mutex<Option<ProcPermit>>,
    /// Last request for one of its files (playlist, init, segment).
    last_used: Mutex<Instant>,
}

impl Variant {
    fn new(id: &str, decision: PlaybackDecision, dir: PathBuf, hw: HwAccel) -> Self {
        Variant {
            meta: None,
            admitted: AtomicBool::new(false),
            permit: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            id: id.to_string(),
            decision,
            dir,
            hw: Mutex::new(hw),
            last_req: AtomicU32::new(0),
            epoch: std::sync::atomic::AtomicU64::new(0),
            run_delta: Mutex::new(None),
            log: LogTail::default(),
            proc: tokio::sync::Mutex::new(Proc {
                child: None,
                start_segment: 0,
                paused: false,
                started: Instant::now(),
            }),
        }
    }

    fn encodes_video(&self) -> bool {
        matches!(self.decision.video, VideoAction::Encode { .. })
    }

    /// Re-encodes the picture or the sound: counts against the transcode caps while admitted.
    fn encodes(&self) -> bool {
        self.encodes_video() || matches!(self.decision.audio, AudioAction::Encode { .. })
    }

    fn used(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// Copy output on a keyframe plan: ffmpeg writes one file per source GOP, assembled into plan
    /// segments by the session.
    fn gop_mode(&self, plan: &SegmentPlan) -> bool {
        plan.is_keyframe_plan() && !self.encodes_video()
    }

    /// An encode on a keyframe plan: IDR frames forced at the plan's boundaries, one file per
    /// segment, each checked against the plan (start and duration).
    fn aligned(&self, plan: &SegmentPlan) -> bool {
        plan.is_keyframe_plan() && self.encodes_video()
    }

    /// A segment's output exists or is in progress.
    fn have_started(&self, plan: &SegmentPlan, n: u32) -> bool {
        if self.dir.join(ffmpeg::segment_name(n)).exists() {
            return true;
        }
        self.gop_mode(plan) && self.dir.join(ffmpeg::gop_name(plan.first_gop(n))).exists()
    }

    /// First segment index >= `start` that has not been started yet.
    fn head(&self, plan: &SegmentPlan, start: u32) -> u32 {
        let mut h = start;
        while h < plan.len() && self.have_started(plan, h) {
            h += 1;
        }
        h
    }
}

struct Session {
    id: String,
    user_id: i64,
    device_id: Option<String>,
    file: FileInfo,
    /// The primary variant's decision (what the session was created with).
    decision: PlaybackDecision,
    /// Session directory (`None` for direct play); the primary variant writes into it.
    dir: Option<PathBuf>,
    started_at: i64,
    plan: SegmentPlan,
    event_mode: bool,
    total_segments: u32,
    source_video_codec: Option<String>,
    source_hdr: Hdr,
    /// What ffmpeg opens: a local path or a loopback upstream-proxy URL (never a provider URL).
    input: String,
    /// Process-budget slot held while the session exists.
    permit: Mutex<Option<ProcPermit>>,
    burn_in: Mutex<Option<BurnIn>>,
    meta: Mutex<Meta>,
    /// Last segment requested on any variant: where a variant that has never run starts when its
    /// `init.mp4` is asked for first (as hls.js does on a level switch).
    last_req: AtomicU32,
    /// Multi-variant session (#111): per-variant admission, idle stop and accounting.
    adaptive: bool,
    /// The user's own transcode limit (re-checked when a variant starts lazily).
    user_max_transcodes: Option<usize>,
    /// Variant of the most recent segment request.
    delivering: Mutex<Option<String>>,
    /// The primary variant first; empty for direct play.
    variants: Vec<Arc<Variant>>,
}

impl Session {
    fn primary(&self) -> Option<&Arc<Variant>> {
        self.variants.first()
    }

    fn variant(&self, id: &str) -> Result<&Arc<Variant>, PlaybackError> {
        self.variants
            .iter()
            .find(|v| v.id == id)
            .ok_or(PlaybackError::SessionNotFound)
    }
}

/// `hw` after runtime degradation: software when the backend failed, without the low-power
/// encoders when they failed ([`crate::hwaccel::VAAPI_LOW_POWER`]).
fn effective(hw: &HwAccel, degraded: &[String]) -> HwAccel {
    if degraded.iter().any(|d| d == hw.name()) {
        return HwAccel::Software;
    }
    if degraded
        .iter()
        .any(|d| d == crate::hwaccel::VAAPI_LOW_POWER)
    {
        if let Some(regular) = hw.without_low_power() {
            return regular;
        }
    }
    hw.clone()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Transcodes started outside the session manager (live TV's shared HLS packagers), counted
/// against the same per-user and global transcode limits.
pub trait ExternalTranscodes: Send + Sync {
    /// `(this user's, server-wide)` transcodes currently running. Called with the admission lock
    /// held ([`SessionManager::admission`]); must not take it again.
    fn counts(&self, user_id: i64) -> (usize, usize);
}

pub struct SessionManager {
    cfg: SessionConfig,
    hw: HwAccel,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Serializes transcode admission between this manager and [`ExternalTranscodes`], so a VOD
    /// transcode and a live one can't both take the user's last slot. Always taken *before* any
    /// other lock of either side.
    admission: Mutex<()>,
    external: Mutex<Option<std::sync::Weak<dyn ExternalTranscodes>>>,
    degraded: Mutex<Vec<String>>,
    indexer: Arc<Indexer>,
    media: MediaCtx,
}

impl SessionManager {
    /// Create a manager. Removes stale transcode directories left by a previous run.
    pub fn new(cfg: SessionConfig, hw: HwAccel) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(cfg.transcode_root());
        let media = MediaCtx::new(cfg.max_ffmpeg_processes);
        Arc::new(SessionManager {
            media,
            cfg,
            hw,
            sessions: Mutex::new(HashMap::new()),
            admission: Mutex::new(()),
            external: Mutex::new(None),
            degraded: Mutex::new(Vec::new()),
            indexer: Arc::new(Indexer::default()),
        })
    }

    /// Shared process budget / upstream proxy / single-flight locks (also used for subtitle
    /// conversion and remote probes).
    pub fn media(&self) -> &MediaCtx {
        &self.media
    }

    pub fn config(&self) -> &SessionConfig {
        &self.cfg
    }

    /// Count `ext`'s transcodes against the limits of new sessions (held weakly).
    pub fn set_external_transcodes(&self, ext: std::sync::Weak<dyn ExternalTranscodes>) {
        *self.external.lock().unwrap() = Some(ext);
    }

    /// The transcode admission lock (see [`ExternalTranscodes`]): hold it while checking the
    /// limits and registering a transcode outside this manager.
    pub fn admission(&self) -> std::sync::MutexGuard<'_, ()> {
        self.admission.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `(this user's, server-wide)` transcode sessions of this manager.
    pub fn transcode_counts(&self, user_id: i64) -> (usize, usize) {
        let map = self.sessions.lock().unwrap();
        let mine = map
            .values()
            .filter(|s| s.user_id == user_id)
            .map(|s| transcode_weight(s))
            .sum();
        (mine, map.values().map(|s| transcode_weight(s)).sum())
    }

    /// Transcode slots still free for `user_id` (the smaller of the global and the per-user
    /// room, live TV included); `usize::MAX` when neither is capped. Decide uses it to choose
    /// how many rungs to list (#111).
    pub fn transcode_room(&self, user_id: i64, user_max: Option<usize>) -> usize {
        let _a = self.admission();
        let (ext_user, ext_global) = self.external_counts(user_id);
        let (mine, all) = self.transcode_counts(user_id);
        let (user_cap, global_cap) = self.transcode_caps(user_max);
        let room = |cap: usize, used: usize| {
            if cap == 0 {
                usize::MAX
            } else {
                cap.saturating_sub(used)
            }
        };
        room(global_cap, all + ext_global).min(room(user_cap, mine + ext_user))
    }

    /// Admit a variant of a multi-variant session before its ffmpeg starts: re-check the
    /// transcode caps (if it re-encodes) and take a process slot. Refused with
    /// [`PlaybackError::VariantRefused`] (429): the player keeps another variant.
    fn admit_variant(&self, s: &Session, v: &Variant) -> Result<(), PlaybackError> {
        if v.admitted.load(Ordering::SeqCst) {
            return Ok(());
        }
        let counts = v.encodes();
        let _a = counts.then(|| self.admission());
        if counts {
            let (ext_user, ext_global) = self.external_counts(s.user_id);
            let (mine, all) = self.transcode_counts(s.user_id);
            let (user_cap, global_cap) = self.transcode_caps(s.user_max_transcodes);
            if global_cap > 0 && all + ext_global >= global_cap {
                return Err(PlaybackError::VariantRefused(format!(
                    "the server's transcode limit ({global_cap}) is reached"
                )));
            }
            if user_cap > 0 && mine + ext_user >= user_cap {
                return Err(PlaybackError::VariantRefused(format!(
                    "your transcode limit ({user_cap}) is reached"
                )));
            }
        }
        let permit = self
            .media
            .try_proc()
            .map_err(|_| PlaybackError::VariantRefused("the server is busy".into()))?;
        *v.permit.lock().unwrap() = Some(permit);
        v.admitted.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Stop a variant's ffmpeg and release its slots (adaptive sessions); its finished segments
    /// stay for a quick return.
    async fn release_variant(&self, v: &Variant, p: &mut Proc) {
        if let Some(mut c) = p.child.take() {
            let _ = c.kill().await;
        }
        p.paused = false;
        v.admitted.store(false, Ordering::SeqCst);
        v.permit.lock().unwrap().take();
    }

    /// `(per-user, global)` transcode caps for a user whose own limit is `user_max`
    /// (`0` = unlimited): the stricter of the server-wide per-user cap and the user's.
    pub fn transcode_caps(&self, user_max: Option<usize>) -> (usize, usize) {
        let user = [self.cfg.max_user_transcodes, user_max.unwrap_or(0)]
            .into_iter()
            .filter(|m| *m > 0)
            .min()
            .unwrap_or(0);
        (user, self.cfg.max_global_transcodes)
    }

    fn external_counts(&self, user_id: i64) -> (usize, usize) {
        let ext = self
            .external
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        ext.map_or((0, 0), |e| e.counts(user_id))
    }

    /// The configured (detected) backend.
    pub fn hwaccel(&self) -> &HwAccel {
        &self.hw
    }

    /// Backend new sessions will use: software once the configured one has been marked degraded,
    /// the regular VAAPI encoders once the low-power ones have.
    pub fn effective_hwaccel(&self) -> HwAccel {
        effective(&self.hw, &self.degraded.lock().unwrap())
    }

    /// Names of backends that failed at runtime and are no longer used.
    pub fn degraded_backends(&self) -> Vec<String> {
        self.degraded.lock().unwrap().clone()
    }

    /// Detection result overlaid with runtime degradation (for the admin hw-accel endpoint).
    pub fn hwaccel_status(&self, detected: &HwAccelStatus) -> HwAccelStatus {
        let mut st = detected.clone();
        st.degraded = self.degraded_backends();
        st.selected = effective(&st.selected, &st.degraded);
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
        self.create_session_with_rungs(req, Vec::new()).await
    }

    /// Like [`SessionManager::create_session`], with extra encoded variants (#111) on the same
    /// segment plan. Rungs start on their first request; they are dropped when the session has
    /// no segment plan (EVENT fallback playlist) or no HLS output.
    pub async fn create_session_with_rungs(
        &self,
        req: StartRequest,
        rungs: Vec<VariantSpec>,
    ) -> Result<SessionInfo, PlaybackError> {
        self.create_inner(req, VARIANT, None, rungs, None).await
    }

    /// A multi-variant session (#111, see [`AdaptiveRequest`]). Only the start variant's ffmpeg
    /// runs at first. Without a keyframe index (EVENT fallback) the rungs are dropped and the
    /// session is a plain single-variant one.
    pub async fn create_session_adaptive(
        &self,
        req: StartRequest,
        a: AdaptiveRequest,
    ) -> Result<SessionInfo, PlaybackError> {
        if !valid_variant_id(&a.primary_id) {
            return Err(PlaybackError::Invalid(format!(
                "bad variant id {:?}",
                a.primary_id
            )));
        }
        if a.start != a.primary_id && !a.rungs.iter().any(|r| r.id == a.start) {
            return Err(PlaybackError::Invalid(format!(
                "unknown start variant {:?}",
                a.start
            )));
        }
        let primary_id = a.primary_id.clone();
        self.create_inner(
            req,
            &primary_id,
            Some(a.primary_meta),
            a.rungs,
            Some(a.start),
        )
        .await
    }

    async fn create_inner(
        &self,
        req: StartRequest,
        primary_id: &str,
        primary_meta: Option<VariantMeta>,
        rungs: Vec<VariantSpec>,
        start_variant: Option<String>,
    ) -> Result<SessionInfo, PlaybackError> {
        let mut ids = vec![primary_id];
        for r in &rungs {
            if !valid_variant_id(&r.id) || ids.contains(&r.id.as_str()) {
                return Err(PlaybackError::Invalid(format!("bad variant id {:?}", r.id)));
            }
            if !matches!(r.decision.video, VideoAction::Encode { .. }) {
                return Err(PlaybackError::Invalid(format!(
                    "variant {} must encode the video",
                    r.id
                )));
            }
            ids.push(&r.id);
        }
        let mode = req.decision.mode;
        let adaptive_req = start_variant.is_some() && mode.needs_ffmpeg();
        let id = uuid::Uuid::new_v4().to_string();
        let seg = self.cfg.segment_secs;
        let duration = req.file.duration_s.unwrap_or(0.0);
        if mode.needs_ffmpeg() && duration <= 0.0 {
            return Err(PlaybackError::Invalid("file duration unknown".into()));
        }
        let mut plan = SegmentPlan::fixed(if mode.needs_ffmpeg() { duration } else { 0.0 }, seg);
        let mut event_mode = false;
        let remote = crate::ffmpeg::is_remote(&req.file.path);
        // ffmpeg never receives a provider URL: remote inputs go through the loopback proxy
        let input = if mode.needs_ffmpeg() && remote {
            self.media
                .upstream()
                .ok_or_else(|| PlaybackError::Invalid("remote playback is not available".into()))?
                .register_with(
                    &req.file.path,
                    &crate::upstream::UpstreamOpts {
                        user_agent: req
                            .user_agent
                            .as_deref()
                            .filter(|ua| !ua.trim().is_empty())
                            .unwrap_or(crate::ffmpeg::REMOTE_UA)
                            .to_string(),
                        // one provider connection per session, also across ffmpeg's seeks
                        exclusive: true,
                        ..Default::default()
                    },
                    self.cfg.upstream_ttl,
                    Some(&id),
                )?
        } else {
            req.file.path.clone()
        };
        if remote && mode.needs_ffmpeg() && req.decision.video == VideoAction::Copy {
            // Remote (provider) input: never scan it for keyframes (that would download the whole
            // file), but read the container's own index (Matroska Cues / MP4 `stss`) with a few
            // ranged requests through the session's exclusive proxy token, before ffmpeg opens
            // it: still one provider connection at a time. With it the session serves a full VOD
            // playlist and seeks restart ffmpeg at the keyframe (#92); without it (no index, no
            // Range support) ffmpeg's growing EVENT playlist is served, which players treat as
            // live and cannot seek in.
            let (dd, fid, url) = (&self.cfg.data_dir, req.file.id, req.file.path.as_str());
            let kfs = match plan::cached_remote_keyframes(dd, fid, url, duration).await {
                Some(k) => Some(k),
                None => {
                    let k = plan::remote_keyframes(&input, self.cfg.remote_index, duration).await;
                    if let Some(k) = &k {
                        plan::store_keyframes(dd, fid, url, k).await;
                    }
                    k
                }
            };
            match kfs {
                Some(k) if !k.is_empty() => plan = SegmentPlan::from_keyframes(&k, duration, seg),
                _ => {
                    tracing::warn!(
                        file = fid,
                        "remote file has no readable keyframe index; serving EVENT playlist (not seekable)"
                    );
                    event_mode = true;
                }
            }
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
        // (burn-in descriptor, subtitle stream index, external file) for text burn-in staging
        let mut stage: Option<(i64, Option<String>)> = None;
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
                let image = is_image_subtitle(s.codec.as_deref().unwrap_or(""));
                if !image {
                    stage = Some((s.idx, s.external_path.clone()));
                }
                Some(BurnIn {
                    ordinal,
                    image,
                    external_path: None, // text subtitles are staged under a safe generated name
                })
            })
        } else {
            None
        };

        // from here on a failure must revoke the tokens again
        let revoke = |m: &Self| {
            if let Some(p) = m.media.upstream() {
                p.revoke_owner(&id);
            }
        };

        if mode.needs_ffmpeg() {
            if let Err(e) = self.ensure_disk_budget().await {
                revoke(self);
                return Err(e);
            }
        }
        // one slot of the global ffmpeg budget per ffmpeg-backed session (remux included);
        // multi-variant sessions take one per running variant instead (`admit_variant`)
        let permit = if mode.needs_ffmpeg() && !adaptive_req {
            match self.media.try_proc() {
                Ok(p) => Some(p),
                Err(e) => {
                    revoke(self);
                    return Err(e);
                }
            }
        } else {
            None
        };

        let dir = mode
            .needs_ffmpeg()
            .then(|| self.cfg.transcode_root().join(&id));
        let mut variants = Vec::new();
        if let Some(dir) = &dir {
            let hw = self.effective_hwaccel();
            let mut primary =
                Variant::new(primary_id, req.decision.clone(), dir.clone(), hw.clone());
            primary.meta = primary_meta;
            variants.push(Arc::new(primary));
            if !event_mode {
                for r in rungs {
                    let vdir = dir.join(&r.id);
                    let mut v = Variant::new(&r.id, r.decision, vdir, hw.clone());
                    v.meta = r.meta;
                    variants.push(Arc::new(v));
                }
            }
        }
        // a multi-variant session needs a plan (its rungs are aligned to it)
        let adaptive = adaptive_req && !event_mode;
        let start_id = start_variant
            .filter(|s| adaptive && variants.iter().any(|v| &v.id == s))
            .unwrap_or_else(|| primary_id.to_string());
        let start_v = variants.iter().find(|v| v.id == start_id).cloned();
        let counts = if adaptive {
            start_v.as_ref().is_some_and(|v| v.encodes())
        } else {
            matches!(mode, PlaybackMode::Transcode | PlaybackMode::AudioTranscode)
        };
        // an EVENT-fallback session asked to be adaptive is a classic one: it takes its permit now
        let permit = match permit {
            None if adaptive_req && !adaptive => match self.media.try_proc() {
                Ok(p) => Some(p),
                Err(e) => {
                    revoke(self);
                    return Err(e);
                }
            },
            p => p,
        };
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
            source_video_codec: video.and_then(|v| v.codec.as_deref().map(normalize_codec)),
            source_hdr: video.map(|v| v.hdr).unwrap_or_default(),
            input,
            permit: Mutex::new(permit),
            burn_in: Mutex::new(burn_in),
            meta: Mutex::new(Meta {
                last_ping: now_unix(),
                last_activity: Instant::now(),
                client_paused: false,
                position_s: req.start_position_s,
                bitrate_kbps: None,
            }),
            last_req: AtomicU32::new(0),
            adaptive,
            user_max_transcodes: req.user_max_transcodes,
            delivering: Mutex::new(None),
            variants,
        });

        {
            // limits are checked and the session registered under one lock: concurrent requests
            // cannot both pass the same check (the admission lock also covers live transcodes)
            let _admission = counts.then(|| self.admission());
            let (ext_user, ext_global) = if counts {
                self.external_counts(req.user_id)
            } else {
                (0, 0)
            };
            let mut map = self.sessions.lock().unwrap();
            if let Some(max) = req.user_max_streams.filter(|m| *m > 0) {
                let mine = map.values().filter(|s| s.user_id == req.user_id).count();
                if mine >= max {
                    drop(map);
                    revoke(self);
                    return Err(PlaybackError::UserStreamLimit(max));
                }
            }
            if counts {
                let global = map.values().map(|s| transcode_weight(s)).sum::<usize>() + ext_global;
                if self.cfg.max_global_transcodes > 0 && global >= self.cfg.max_global_transcodes {
                    drop(map);
                    revoke(self);
                    return Err(PlaybackError::GlobalLimit(self.cfg.max_global_transcodes));
                }
                let user = map
                    .values()
                    .filter(|s| s.user_id == req.user_id)
                    .map(|s| transcode_weight(s))
                    .sum::<usize>()
                    + ext_user;
                let (user_max, _) = self.transcode_caps(req.user_max_transcodes);
                if user_max > 0 && user >= user_max {
                    drop(map);
                    revoke(self);
                    return Err(PlaybackError::UserLimit(user_max));
                }
            }
            if let (true, Some(v)) = (adaptive, &start_v) {
                // the start variant is admitted with the session (same lock, so the caps hold)
                match self.media.try_proc() {
                    Ok(p) => *v.permit.lock().unwrap() = Some(p),
                    Err(e) => {
                        drop(map);
                        revoke(self);
                        return Err(e);
                    }
                }
                v.admitted.store(true, Ordering::SeqCst);
            }
            map.insert(id.clone(), session.clone());
        }
        if event_mode && !remote {
            self.indexer
                .acquire(&self.cfg, session.file.id, &session.file.path);
        }

        if let (Some(dir), Some(primary)) = (&dir, start_v.as_ref()) {
            // map the resume position through the plan (real keyframe boundaries in copy mode)
            let start = session
                .plan
                .segment_at(req.start_position_s.max(0.0))
                .min(total_segments.saturating_sub(1));
            session.last_req.store(start, Ordering::Relaxed);
            for v in &session.variants {
                v.last_req.store(start, Ordering::Relaxed);
            }
            let res = async {
                tokio::fs::create_dir_all(dir).await?;
                if let Some((idx, external)) = &stage {
                    self.stage_burn_in(&session, dir, *idx, external.as_deref())
                        .await?;
                }
                let mut p = primary.proc.lock().await;
                self.start_ffmpeg(&session, primary, &mut p, start).await
            }
            .await;
            if let Err(e) = res {
                self.remove(&id).await;
                return Err(e);
            }
        }
        if dir.is_some() {
            self.spawn_throttle(Arc::downgrade(&session), self.cfg.max_session_disk_bytes);
        }
        Ok(self.info_of(&session).await)
    }

    /// Extract a text subtitle track into the session directory under a fixed, safe name
    /// (`burn.ass`) so the `subtitles=` filter never sees user-controlled paths.
    async fn stage_burn_in(
        &self,
        s: &Session,
        dir: &Path,
        stream_idx: i64,
        external: Option<&str>,
    ) -> Result<(), PlaybackError> {
        let out = dir.join("burn.ass");
        let (src, map) = match external {
            Some(p) if !crate::ffmpeg::is_remote(p) => (p.to_string(), "0:s:0".to_string()),
            Some(_) => {
                return Err(PlaybackError::Invalid(
                    "remote subtitle files cannot be burned in".into(),
                ));
            }
            None => (s.input.clone(), format!("0:{stream_idx}")),
        };
        let remote_in = crate::ffmpeg::is_remote(&src);
        let mut cmd = Command::new(&self.cfg.ffmpeg_path);
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
        cmd.args([
            "-protocol_whitelist",
            crate::ffmpeg::protocol_whitelist(remote_in),
        ]);
        cmd.arg("-i")
            .arg(&src)
            .args(["-map", &map, "-c:s", "ass"])
            .arg(&out);
        let output = tokio::time::timeout(
            Duration::from_secs(180),
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| PlaybackError::ffmpeg("subtitle staging timed out"))??;
        if !output.status.success() || !out.exists() {
            return Err(PlaybackError::ffmpeg(format!(
                "subtitle staging failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        *s.burn_in.lock().unwrap() = Some(BurnIn {
            ordinal: 0,
            image: false,
            external_path: Some(out.to_string_lossy().into_owned()),
        });
        Ok(())
    }

    /// Sum of all session scratch directories.
    pub async fn disk_usage(&self) -> u64 {
        dir_size(&self.cfg.transcode_root()).await
    }

    /// Keep `data_dir/transcode` within its budget: evict idle sessions oldest-first; refuse
    /// (`DiskBudget`) while still over.
    async fn ensure_disk_budget(&self) -> Result<(), PlaybackError> {
        let max = self.cfg.max_total_disk_bytes;
        if max == 0 {
            return Ok(());
        }
        loop {
            if dir_size(&self.cfg.transcode_root()).await <= max {
                return Ok(());
            }
            let victim = {
                let map = self.sessions.lock().unwrap();
                map.values()
                    .filter(|s| s.dir.is_some())
                    .filter_map(|s| {
                        let idle = s.meta.lock().unwrap().last_activity.elapsed();
                        (idle >= self.cfg.disk_evict_min_idle).then(|| (idle, s.id.clone()))
                    })
                    .max_by_key(|(idle, _)| *idle)
                    .map(|(_, id)| id)
            };
            match victim {
                Some(id) => {
                    tracing::warn!(session = %id, "scratch disk over budget: evicting idle session");
                    self.remove(&id).await;
                }
                None => return Err(PlaybackError::DiskBudget),
            }
        }
    }

    /// Per-session task pausing ffmpeg when it runs too far ahead of the client.
    fn spawn_throttle(&self, weak: Weak<Session>, session_disk_cap: u64) {
        let max_ahead = self.cfg.max_ahead_segments;
        let resume_at = max_ahead - (max_ahead / 4).max(1).min(max_ahead);
        let interval = self.cfg.throttle_interval;
        tokio::spawn(async move {
            let mut tick = 0u32;
            loop {
                tokio::time::sleep(interval).await;
                let Some(s) = weak.upgrade() else { break };
                let Some(dir) = s.dir.clone() else { break };
                tick = tick.wrapping_add(1);
                if tick % 10 == 0 && session_disk_cap > 0 && dir_size(&dir).await > session_disk_cap
                {
                    // bounded scratch: drop segments well behind each variant's last request (a
                    // seek back simply restarts ffmpeg there)
                    for v in &s.variants {
                        let keep_from = v.last_req.load(Ordering::Relaxed).saturating_sub(10);
                        trim_behind(&v.dir, keep_from).await;
                    }
                }
                if max_ahead == 0 {
                    continue;
                }
                for v in &s.variants {
                    let mut p = v.proc.lock().await;
                    let alive = p
                        .child
                        .as_mut()
                        .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
                    if !alive {
                        continue;
                    }
                    let head = v.head(&s.plan, p.start_segment);
                    let ahead = head
                        .saturating_sub(v.last_req.load(Ordering::Relaxed).max(p.start_segment));
                    if !p.paused && ahead > max_ahead {
                        if let Some(c) = p.child.as_ref() {
                            tracing::debug!(session = %s.id, variant = %v.id, ahead, "pausing ffmpeg (read-ahead limit)");
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
            }
        });
    }

    async fn info_of(&self, s: &Session) -> SessionInfo {
        // (alive, paused) per variant; a variant busy (re)starting counts as running
        let states: Vec<(bool, bool)> = s
            .variants
            .iter()
            .map(|v| match v.proc.try_lock() {
                Ok(mut p) => (
                    p.child
                        .as_mut()
                        .is_some_and(|c| matches!(c.try_wait(), Ok(None))),
                    p.paused,
                ),
                Err(_) => (true, false),
            })
            .collect();
        let delivering = s.delivering.lock().unwrap().clone();
        // the variant that describes the session: what is being delivered, else the primary
        let shown = delivering
            .as_deref()
            .and_then(|d| s.variants.iter().position(|v| v.id == d))
            .unwrap_or(0);
        let (active, paused) = if s.adaptive {
            let alive: Vec<&(bool, bool)> = states.iter().filter(|x| x.0).collect();
            (
                !alive.is_empty(),
                !alive.is_empty() && alive.iter().all(|x| x.1),
            )
        } else {
            states.first().copied().unwrap_or((false, false))
        };
        let shown_v = s.variants.get(shown);
        let decision = shown_v.map_or(&s.decision, |v| &v.decision);
        // clone: a guard taken inside the struct literal would live until its end (and deadlock)
        let hw = shown_v
            .map(|v| v.hw.lock().unwrap().clone())
            .unwrap_or_else(|| self.effective_hwaccel());
        let variants = if s.adaptive {
            s.variants
                .iter()
                .zip(&states)
                .map(|(v, (alive, paused))| {
                    let vhw = v.hw.lock().unwrap().clone();
                    let meta = v.meta.clone();
                    VariantStatus {
                        id: v.id.clone(),
                        label: meta
                            .as_ref()
                            .map_or_else(|| v.id.clone(), |m| m.label.clone()),
                        width: meta.as_ref().map_or(0, |m| m.width),
                        height: meta.as_ref().map_or(0, |m| m.height),
                        bandwidth_kbps: meta.as_ref().map_or(0, |m| m.average_kbps),
                        encoder: match &v.decision.video {
                            VideoAction::Encode { codec, .. } => Some(vhw.encoder_label(codec)),
                            _ => None,
                        },
                        state: match (alive, paused) {
                            (true, true) => "paused",
                            (true, false) => "running",
                            _ => "idle",
                        }
                        .into(),
                        last_request_s: v.last_used.lock().unwrap().elapsed().as_secs(),
                    }
                })
                .collect()
        } else {
            Vec::new()
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
            bitrate_kbps: if s.adaptive {
                shown_v
                    .and_then(|v| v.meta.as_ref())
                    .map(|m| m.average_kbps)
                    .or(m.bitrate_kbps)
            } else {
                m.bitrate_kbps.or_else(|| match &s.decision.video {
                    VideoAction::Encode { bitrate_kbps, .. } => Some(*bitrate_kbps),
                    _ => s.file.bitrate.map(|b| (b / 1000) as u32),
                })
            },
            hwaccel: hw.name().into(),
            encoder: match &decision.video {
                VideoAction::Encode { codec, .. } => Some(hw.encoder_label(codec)),
                _ => None,
            },
            active,
            paused,
            client_paused: m.client_paused,
            fallback_playlist: s.event_mode,
            variants,
            delivering: if s.adaptive { delivering } else { None },
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

    /// Keep a session alive: any request for its media (segments, playlists, proxied bytes) from
    /// any client counts, e.g. a Chromecast fetching while the sender only pings progress.
    /// Media requests don't change the paused state: a paused player still fills its buffer.
    pub fn touch_session(&self, id: &str) {
        if let Ok(s) = self.get(id) {
            s.meta.lock().unwrap().last_activity = Instant::now();
        }
    }

    /// Progress ping from the client; also keeps the session alive. `paused`: the player is paused,
    /// so the session may stay idle up to `paused_idle_timeout` (#95).
    pub fn update_progress(
        &self,
        id: &str,
        position_s: f64,
        bitrate_kbps: Option<u32>,
        paused: bool,
    ) -> Result<(), PlaybackError> {
        let s = self.get(id)?;
        let mut m = s.meta.lock().unwrap();
        m.last_ping = now_unix();
        m.last_activity = Instant::now();
        m.client_paused = paused;
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
        for v in &s.variants {
            let mut p = v.proc.lock().await;
            if let Some(mut c) = p.child.take() {
                let _ = c.kill().await;
            }
            v.admitted.store(false, Ordering::SeqCst);
            v.permit.lock().unwrap().take();
        }
        if let Some(dir) = &s.dir {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        if s.event_mode && !crate::ffmpeg::is_remote(&s.file.path) {
            self.indexer.release(s.file.id);
        }
        if let Some(p) = self.media.upstream() {
            p.revoke_owner(id);
        }
        s.permit.lock().unwrap().take(); // free the ffmpeg process slot right away
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

    /// How long a session may go without activity before it is reaped. Direct play keeps
    /// `direct_idle_timeout` whatever the player does (no ffmpeg to keep; a VOD direct-play
    /// response holds a provider connection, A25).
    fn idle_limit(&self, mode: PlaybackMode, client_paused: bool) -> Duration {
        if !mode.needs_ffmpeg() {
            self.cfg.direct_idle_timeout
        } else if client_paused {
            self.cfg.idle_timeout.max(self.cfg.paused_idle_timeout)
        } else {
            self.cfg.idle_timeout
        }
    }

    /// Kill sessions idle beyond their timeout; returns the reaped ids.
    pub async fn reap_idle(&self) -> Vec<String> {
        let victims: Vec<String> = {
            let map = self.sessions.lock().unwrap();
            map.values()
                .filter(|s| {
                    let m = s.meta.lock().unwrap();
                    m.last_activity.elapsed() >= self.idle_limit(s.decision.mode, m.client_paused)
                })
                .map(|s| s.id.clone())
                .collect()
        };
        for id in &victims {
            tracing::info!(session = %id, "reaping idle playback session");
            self.remove(id).await;
        }
        self.stop_idle_variants().await;
        victims
    }

    /// Multi-variant sessions: stop each admitted variant the player hasn't fetched from for
    /// `variant_idle_timeout`, releasing its transcode and process slots (#111).
    pub async fn stop_idle_variants(&self) -> Vec<(String, String)> {
        let limit = self.cfg.variant_idle_timeout;
        let candidates: Vec<(Arc<Session>, Arc<Variant>)> = {
            let map = self.sessions.lock().unwrap();
            map.values()
                .filter(|s| s.adaptive)
                .flat_map(|s| {
                    s.variants
                        .iter()
                        .filter(|v| {
                            v.admitted.load(Ordering::SeqCst)
                                && v.last_used.lock().unwrap().elapsed() >= limit
                        })
                        .map(|v| (s.clone(), v.clone()))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let mut stopped = Vec::new();
        for (s, v) in candidates {
            let mut p = v.proc.lock().await;
            // a request may have arrived while we waited for the lock
            if !v.admitted.load(Ordering::SeqCst) || v.last_used.lock().unwrap().elapsed() < limit {
                continue;
            }
            tracing::info!(session = %s.id, variant = %v.id, "stopping idle variant");
            self.release_variant(&v, &mut p).await;
            stopped.push((s.id.clone(), v.id.clone()));
        }
        stopped
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
        if s.adaptive {
            let mut vs: Vec<(&str, &VariantMeta)> = s
                .variants
                .iter()
                .filter_map(|v| Some((v.id.as_str(), v.meta.as_ref()?)))
                .collect();
            vs.sort_by_key(|(_, m)| m.peak_kbps);
            return Ok(build_multivariant_playlist(&vs));
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
        let primary = s.primary().map_or(VARIANT, |v| v.id.as_str());
        Ok(build_master_playlist_for(
            primary,
            ((vb + ab) as u64) * 1000,
            w.zip(h),
        ))
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
                .ok_or_else(|| PlaybackError::segment_unavailable("playlist not ready"));
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
            let primary = s.primary().ok_or(PlaybackError::SessionNotFound)?;
            let failed = {
                let mut p = primary.proc.lock().await;
                matches!(p.child.as_mut().map(|c| c.try_wait()), Some(Ok(Some(st))) if !st.success())
            };
            if failed || Instant::now() >= deadline {
                return Err(PlaybackError::segment_unavailable(format!(
                    "no playlist from ffmpeg: {}",
                    primary.log.tail(5)
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn touch(&self, s: &Session) {
        s.meta.lock().unwrap().last_activity = Instant::now();
    }

    // ---------------- segments ----------------

    /// Media playlist of one variant (all variants share the session's plan, so the same text).
    pub async fn variant_playlist(&self, id: &str, variant: &str) -> Result<String, PlaybackError> {
        self.get(id)?.variant(variant)?.used();
        self.media_playlist_async(id).await
    }

    /// Ids of the session's variants, primary first (empty for direct play).
    pub fn variant_ids(&self, id: &str) -> Result<Vec<String>, PlaybackError> {
        Ok(self
            .get(id)?
            .variants
            .iter()
            .map(|v| v.id.clone())
            .collect())
    }

    /// Output directory of one variant.
    pub fn variant_dir(&self, id: &str, variant: &str) -> Result<PathBuf, PlaybackError> {
        Ok(self.get(id)?.variant(variant)?.dir.clone())
    }

    /// Whether the variant's ffmpeg is currently running (or paused by the throttle).
    pub async fn variant_running(&self, id: &str, variant: &str) -> Result<bool, PlaybackError> {
        let s = self.get(id)?;
        let v = s.variant(variant)?;
        let mut p = v.proc.lock().await;
        Ok(p.child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None))))
    }

    /// Resolve `init.mp4` / `seg_NNNNN.m4s` of the primary variant (see
    /// [`SessionManager::variant_segment_path`]).
    pub async fn segment_path(&self, id: &str, name: &str) -> Result<PathBuf, PlaybackError> {
        self.variant_segment_path(id, VARIANT, name).await
    }

    /// Resolve `init.mp4` / `seg_NNNNN.m4s` of a variant to an existing file, (re)starting that
    /// variant's ffmpeg as needed (a variant that isn't running starts here).
    pub async fn variant_segment_path(
        &self,
        id: &str,
        variant: &str,
        name: &str,
    ) -> Result<PathBuf, PlaybackError> {
        let s = self.get(id)?;
        self.touch(&s);
        if s.dir.is_none() {
            return Err(PlaybackError::Invalid(
                "no HLS output for this session".into(),
            ));
        }
        let v = s.variant(variant)?.clone();
        v.used();
        let dir = v.dir.clone();
        let wanted: Option<u32> = if name == INIT_NAME {
            None
        } else {
            let n = ffmpeg::parse_segment_name(name)
                .ok_or_else(|| PlaybackError::Invalid(format!("bad segment name {name}")))?;
            if n >= s.total_segments {
                return Err(PlaybackError::Invalid("segment beyond end of media".into()));
            }
            v.last_req.store(n, Ordering::Relaxed);
            s.last_req.store(n, Ordering::Relaxed);
            *s.delivering.lock().unwrap() = Some(v.id.clone());
            Some(n)
        };
        let path = dir.join(name);
        let deadline = Instant::now() + self.cfg.segment_wait_timeout;
        let mut restarts = 0;
        let mut force_restart = false;
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
                // segments are written via temp file + rename (or assembled likewise): complete.
                // Re-validate on every serve: only clean [styp] moof/mdat bytes may reach MSE.
                let n = wanted.unwrap_or(0);
                let epoch = v.epoch.load(Ordering::SeqCst);
                let expected = if s.event_mode || !v.encodes_video() {
                    None // copy-mode files were normalised when assembled; fallback has no grid
                } else {
                    Some(Expected {
                        start: s.plan.start(n),
                        seq: n * 16 + 1,
                        // aligned rungs must cover exactly the plan segment (the last one may
                        // end with the file's audio, so only its start is checked)
                        len: (v.aligned(&s.plan) && n + 1 < s.total_segments)
                            .then(|| s.plan.seg_len(n)),
                        check_start: v.aligned(&s.plan),
                    })
                };
                let valid = {
                    match normalize_segment(&dir, &path, expected, &v, epoch).await {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!(session = %id, segment = n, error = %e, "discarding invalid segment");
                            let _ = tokio::fs::remove_file(&path).await;
                            false
                        }
                    }
                };
                if valid {
                    self.touch(&s);
                    return Ok(path);
                }
                force_restart = true;
            }
            if let (Some(n), true) = (wanted, v.gop_mode(&s.plan)) {
                match assemble_segment(&dir, &s, &v, n).await {
                    Ok(true) => return Ok(path),
                    Ok(false) => {}
                    Err(AssembleError::Invalid(e)) => {
                        tracing::warn!(session = %id, segment = n, error = %e, "discarding invalid GOP data");
                        force_restart = true;
                    }
                    Err(AssembleError::Fatal(e)) => return Err(e),
                }
            }
            {
                let mut p = v.proc.lock().await;
                let state = p.child.as_mut().map(|c| c.try_wait());
                let alive = matches!(state, Some(Ok(None)));
                let failed = matches!(state, Some(Ok(Some(st))) if !st.success());
                // an init request for a variant that has never run starts it where the player is
                let target = wanted.unwrap_or(if p.child.is_none() {
                    s.last_req.load(Ordering::Relaxed)
                } else {
                    p.start_segment
                });
                let mut fallback = false;
                if failed {
                    let cur = v.hw.lock().unwrap().clone();
                    let codec = match &v.decision.video {
                        VideoAction::Encode { codec, .. } => Some(codec.as_str()),
                        _ => None,
                    };
                    if let Some(codec) = codec.filter(|_| {
                        cur != HwAccel::Software
                            && p.started.elapsed() < self.cfg.hw_startup_grace
                            && !v.have_started(&s.plan, p.start_segment)
                    }) {
                        // first step down from the low-power encoder to the regular one, then to
                        // software (#113)
                        let (degrade, next) = match cur.without_low_power() {
                            Some(regular) if cur.low_power().for_codec(codec) => {
                                (crate::hwaccel::VAAPI_LOW_POWER, regular)
                            }
                            _ => (cur.name(), HwAccel::Software),
                        };
                        tracing::warn!(
                            session = %id,
                            variant = %v.id,
                            backend = cur.name(),
                            failed = degrade,
                            next = %next.encoder_label(codec),
                            "hardware ffmpeg failed before producing output; retrying: {}",
                            v.log.tail(5)
                        );
                        {
                            let mut d = self.degraded.lock().unwrap();
                            if !d.iter().any(|x| x == degrade) {
                                d.push(degrade.to_string());
                            }
                        }
                        *v.hw.lock().unwrap() = next;
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
                let must_restart = if fallback || force_restart {
                    true
                } else if alive {
                    match wanted {
                        None => false,
                        Some(n) => {
                            let head = v.head(&s.plan, p.start_segment);
                            n < p.start_segment || n > head + self.cfg.lookahead_segments
                        }
                    }
                } else {
                    true
                };
                if must_restart {
                    if !fallback {
                        if restarts >= 2 {
                            return Err(PlaybackError::segment_unavailable(format!(
                                "ffmpeg failed: {}",
                                v.log.tail(5)
                            )));
                        }
                        restarts += 1;
                    }
                    if s.adaptive {
                        // a variant that was idle (never started, or stopped) is admitted again:
                        // caps and process budget re-checked (429 → the player switches)
                        self.admit_variant(&s, &v)?;
                    }
                    tracing::info!(session = %id, variant = %v.id, segment = target, "restarting ffmpeg");
                    self.start_ffmpeg(&s, &v, &mut p, target).await?;
                    force_restart = false;
                }
            }
            if Instant::now() >= deadline {
                return Err(PlaybackError::segment_unavailable(format!(
                    "timeout waiting for {name}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn start_ffmpeg(
        &self,
        s: &Session,
        v: &Variant,
        p: &mut Proc,
        start_segment: u32,
    ) -> Result<(), PlaybackError> {
        if let Some(mut c) = p.child.take() {
            let _ = c.kill().await;
        }
        p.paused = false;
        let dir = &v.dir;
        tokio::fs::create_dir_all(dir).await?;
        // everything this run will (re)produce must come from *this* run: drop leftovers of older
        // runs from `start_segment` on (init and earlier segments stay valid)
        v.epoch.fetch_add(1, Ordering::SeqCst);
        *v.run_delta.lock().unwrap() = None;
        remove_stale(dir, start_segment, v.gop_mode(&s.plan)).await;
        let hw = v.hw.lock().unwrap().clone();
        let params = BuildParams {
            input: &s.input,
            decision: &v.decision,
            hwaccel: &hw,
            source_video_codec: s.source_video_codec.as_deref(),
            source_hdr: s.source_hdr,
            burn_in: if v.decision.subtitle == SubtitleAction::BurnIn {
                s.burn_in.lock().unwrap().clone()
            } else {
                None
            },
            start_segment,
            segment_secs: self.cfg.segment_secs,
            out_dir: dir,
        };
        // encodes always follow the plan's keyframe times (fixed or source keyframes); copy needs
        // a keyframe plan (else it's the EVENT fallback)
        let args = if v.gop_mode(&s.plan) || (v.encodes_video() && !s.event_mode) {
            ffmpeg::build_args_with_plan(&params, &s.plan)
        } else {
            ffmpeg::build_args(&params)
        };
        let mut child = Command::new(&self.cfg.ffmpeg_path)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| PlaybackError::ffmpeg("cannot start ffmpeg"))?;
        // stderr may contain URLs, credentials and paths: it is redacted line by line before it is
        // stored anywhere, and only a bounded tail is kept in memory
        if let Some(err) = child.stderr.take() {
            let weak_log = v.log.handle();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut err = err;
                let mut buf = [0u8; 4096];
                let mut cur: Vec<u8> = Vec::new();
                // read everything (a full pipe would stall ffmpeg) but keep only bounded lines
                while let Ok(n) = err.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            if !cur.is_empty() {
                                weak_log.push(&String::from_utf8_lossy(&cur));
                                cur.clear();
                            }
                        } else if cur.len() < 1024 {
                            cur.push(b);
                        }
                    }
                }
                if !cur.is_empty() {
                    weak_log.push(&String::from_utf8_lossy(&cur));
                }
            });
        }
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

/// Delete output of earlier ffmpeg runs that the run starting at `start_segment` will replace.
/// `copy_plan`: the variant writes GOP files (copy output on a keyframe plan).
async fn remove_stale(dir: &Path, start_segment: u32, copy_plan: bool) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        // unassembled GOP files of older runs are dropped entirely: they carry another run's
        // timeline shift and are regenerated on demand
        let stale = name.ends_with(".tmp")
            || ffmpeg::parse_segment_name(&name).is_some_and(|k| k >= start_segment)
            || (copy_plan && ffmpeg::parse_gop_name(&name).is_some());
        if stale {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
}

/// Timeline shift (plan time − actual time) of the current ffmpeg run, measured once on the first
/// fragment processed and then reused for the whole run. `None` if ffmpeg was restarted meanwhile.
fn run_shift(
    cell: &Mutex<Option<(u64, f64)>>,
    epoch: &std::sync::atomic::AtomicU64,
    epoch_seen: u64,
    data: &[u8],
    tracks: &HashMap<u32, fmp4::Track>,
    plan_time: f64,
) -> Option<f64> {
    let mut g = cell.lock().unwrap();
    if let Some((e, d)) = *g {
        if e == epoch_seen {
            return Some(d);
        }
    }
    if epoch.load(Ordering::SeqCst) != epoch_seen {
        return None;
    }
    let mut d = plan_time - fmp4::first_video_pts(data, tracks)?;
    // never push the video decode time below zero (the very first fragment of a file starts
    // `B-frame delay` late; that offset is then kept for the whole run, consistently)
    d = d.max(-fmp4::first_video_tfdt(data, tracks)?);
    *g = Some((epoch_seen, d));
    Some(d)
}

/// Where an encoded segment must sit on the plan.
#[derive(Clone, Copy)]
struct Expected {
    /// Plan start (seconds) of the segment: its first video sample is moved there.
    start: f64,
    /// `mfhd` sequence number of the first fragment.
    seq: u32,
    /// Aligned rungs: the plan length the segment's video must cover.
    len: Option<f64>,
    /// Aligned rungs: the segment's own start (on the run's timeline) must be the plan start, so
    /// a keyframe the encoder added by itself (a split segment) is caught instead of shifting
    /// every later boundary.
    check_start: bool,
}

/// How far an aligned rung's segment start or length may be off the plan: under one frame at
/// ≥ 20 fps, well under any stray split.
const ALIGN_TOLERANCE_S: f64 = 0.06;

/// Validate an ffmpeg-written segment. With `expected` (encoded output) it is also rewritten
/// to `[styp] moof/mdat…` (no `sidx`) with its first video sample exactly at the plan start.
/// Discarded (rename skipped) if ffmpeg was restarted meanwhile.
async fn normalize_segment(
    dir: &Path,
    path: &Path,
    expected: Option<Expected>,
    sess: &Variant,
    epoch_seen: u64,
) -> Result<(), String> {
    let epoch = &sess.epoch;
    let init = ready_init(dir).await.ok_or("init not ready")?;
    let init = tokio::fs::read(init).await.map_err(|e| e.to_string())?;
    let data = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
    let tracks = fmp4::init_tracks(&init);
    let cleaned = fmp4::clean_segment(&data, &tracks)?;
    let Some(exp) = expected else {
        return Ok(());
    };
    let (expected, expected_seq) = (exp.start, exp.seq);
    let mut out = cleaned.styp.unwrap_or_default();
    out.extend(cleaned.body);
    let delta = run_shift(&sess.run_delta, epoch, epoch_seen, &out, &tracks, expected)
        .ok_or("ffmpeg restarted")?;
    if exp.check_start {
        let actual = fmp4::first_video_pts(&out, &tracks).ok_or("no video")? + delta;
        if (actual - expected).abs() > ALIGN_TOLERANCE_S {
            return Err(format!(
                "segment starts at {actual:.3}s, plan says {expected:.3}s (unforced keyframe?)"
            ));
        }
    }
    if let (Some(want), Some(got)) = (exp.len, fmp4::video_duration(&out, &tracks)) {
        if (got - want).abs() > ALIGN_TOLERANCE_S {
            return Err(format!(
                "segment covers {got:.3}s, plan says {want:.3}s (unforced keyframe?)"
            ));
        }
    }
    if delta.abs() >= 0.0005 && !fmp4::shift(&mut out, &tracks, delta) {
        tracing::warn!("could not normalise fragment timestamps");
    }
    fmp4::renumber(&mut out, expected_seq);
    if out != data && epoch.load(Ordering::SeqCst) == epoch_seen {
        let tmp = path.with_extension("m4s.tmp");
        tokio::fs::write(&tmp, &out)
            .await
            .map_err(|e| e.to_string())?;
        if epoch.load(Ordering::SeqCst) == epoch_seen {
            tokio::fs::rename(&tmp, path)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }
    Ok(())
}

enum AssembleError {
    /// GOP data is unusable (deleted; the caller should restart ffmpeg).
    Invalid(String),
    Fatal(PlaybackError),
}

impl From<std::io::Error> for AssembleError {
    fn from(e: std::io::Error) -> Self {
        AssembleError::Fatal(e.into())
    }
}

/// Concatenate the GOP files of plan segment `n` into `seg_n.m4s` once all exist: each GOP is
/// validated (`[styp] [sidx] moof mdat` only, tracks known to the init) and retimed onto the
/// index timeline; the segment is `styp` + all `moof/mdat` pairs.
async fn assemble_segment(
    dir: &Path,
    s: &Session,
    sess: &Variant,
    n: u32,
) -> Result<bool, AssembleError> {
    let (plan, epoch) = (&s.plan, &sess.epoch);
    let epoch_seen = epoch.load(Ordering::SeqCst);
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
    let mut styp = None;
    let mut out = Vec::new();
    for g in a..b {
        let raw = tokio::fs::read(dir.join(ffmpeg::gop_name(g))).await?;
        let cleaned = match fmp4::clean_segment(&raw, &tracks) {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(dir.join(ffmpeg::gop_name(g))).await;
                return Err(AssembleError::Invalid(format!("gop {g}: {e}")));
            }
        };
        if styp.is_none() {
            styp = cleaned.styp;
        }
        let mut body = cleaned.body;
        fmp4::renumber(&mut body, g * 16 + 1);
        if let Some(t) = plan.gop_time(g) {
            // a run-dependent timeline shift (B-frame delay, audio priming) would make restarted
            // segments land off the playlist timeline; shift the whole run onto the index time
            let Some(d) = run_shift(&sess.run_delta, epoch, epoch_seen, &body, &tracks, t) else {
                return Ok(false);
            };
            let actual = fmp4::first_video_pts(&body, &tracks).unwrap_or(t) + d;
            if (actual - t).abs() > 1.0 {
                let _ = tokio::fs::remove_file(dir.join(ffmpeg::gop_name(g))).await;
                return Err(AssembleError::Fatal(PlaybackError::segment_unavailable(
                    format!(
                        "keyframe index does not match the media (gop {g} is {:+.2}s off)",
                        actual - t
                    ),
                )));
            }
            if d.abs() >= 0.0005 && !fmp4::shift(&mut body, &tracks, d) {
                tracing::warn!(
                    segment = n,
                    gop = g,
                    "could not normalise fragment timestamps"
                );
            }
        }
        out.extend(body);
    }
    let mut seg = styp.unwrap_or_default();
    seg.extend(out);
    if epoch.load(Ordering::SeqCst) != epoch_seen {
        return Ok(false); // ffmpeg restarted while we worked: do not publish old-run data
    }
    let tmp = dir.join(format!("{}.tmp", ffmpeg::segment_name(n)));
    tokio::fs::write(&tmp, seg).await?;
    if epoch.load(Ordering::SeqCst) != epoch_seen {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Ok(false);
    }
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

/// Bounded, redacted tail of a subprocess' stderr.
#[derive(Default)]
struct LogTail(Arc<Mutex<std::collections::VecDeque<String>>>);

#[derive(Clone)]
struct LogHandle(Arc<Mutex<std::collections::VecDeque<String>>>);

impl LogTail {
    fn handle(&self) -> LogHandle {
        LogHandle(self.0.clone())
    }
    /// Last lines, joined (already redacted).
    fn tail(&self, n: usize) -> String {
        let l = self.0.lock().unwrap();
        l.iter()
            .rev()
            .take(n)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

impl LogHandle {
    fn push(&self, line: &str) {
        let mut red = redact_text(line);
        red.truncate(300);
        let mut l = self.0.lock().unwrap();
        l.push_back(red);
        while l.len() > 64 {
            l.pop_front();
        }
    }
}

/// Total size of regular files under `root` (one level of session directories).
async fn dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&d).await else {
            continue;
        };
        while let Ok(Some(e)) = rd.next_entry().await {
            match e.metadata().await {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// Remove assembled/encoded segments (and GOP leftovers) with index below `keep_from`.
async fn trim_behind(dir: &Path, keep_from: u32) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = ffmpeg::parse_segment_name(&name).is_some_and(|k| k < keep_from)
            || ffmpeg::parse_gop_name(&name).is_some_and(|k| k < keep_from);
        if old {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
}

pub fn build_master_playlist(bandwidth_bps: u64, resolution: Option<(u32, u32)>) -> String {
    build_master_playlist_for(VARIANT, bandwidth_bps, resolution)
}

/// Single-variant master pointing at variant `id`.
pub fn build_master_playlist_for(
    id: &str,
    bandwidth_bps: u64,
    resolution: Option<(u32, u32)>,
) -> String {
    let res = resolution
        .map(|(w, h)| format!(",RESOLUTION={w}x{h}"))
        .unwrap_or_default();
    format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-STREAM-INF:BANDWIDTH={bandwidth_bps}{res}\n{id}/index.m3u8\n"
    )
}

/// Master playlist of a multi-variant session (#111): one `EXT-X-STREAM-INF` per variant, in the
/// given order (ascending bandwidth), with both bandwidth figures (hls.js uses the average while
/// its buffer is healthy, the peak when it's low) and `CODECS` (hls.js drops a level it can't
/// play, and switches only between levels of one codec set).
pub fn build_multivariant_playlist(variants: &[(&str, &VariantMeta)]) -> String {
    let mut o = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    for (id, m) in variants {
        o.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={}",
            m.peak_kbps.max(m.average_kbps) as u64 * 1000,
            m.average_kbps as u64 * 1000
        ));
        if m.width > 0 && m.height > 0 {
            o.push_str(&format!(",RESOLUTION={}x{}", m.width, m.height));
        }
        if let Some(c) = &m.codecs {
            o.push_str(&format!(",CODECS=\"{c}\""));
        }
        o.push_str(&format!("\n{id}/index.m3u8\n"));
    }
    o
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

/// Transcode slots a session holds: one for a classic (audio-)transcode session for its whole
/// life; for a multi-variant session, one per admitted variant that re-encodes something.
fn transcode_weight(s: &Session) -> usize {
    if s.adaptive {
        s.variants
            .iter()
            .filter(|v| v.admitted.load(Ordering::SeqCst) && v.encodes())
            .count()
    } else {
        usize::from(counts_as_transcode(s))
    }
}

fn counts_as_transcode(s: &Session) -> bool {
    matches!(
        s.decision.mode,
        PlaybackMode::Transcode | PlaybackMode::AudioTranscode
    )
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
    fn multivariant_master_shape() {
        let m = |label: &str, w, h, avg, peak, codecs: Option<&str>| VariantMeta {
            label: label.into(),
            width: w,
            height: h,
            average_kbps: avg,
            peak_kbps: peak,
            codecs: codecs.map(str::to_string),
        };
        let low = m(
            "480p",
            854,
            462,
            1592,
            1872,
            Some("hvc1.2.4.L90.B0,mp4a.40.2"),
        );
        let orig = m("Original", 1920, 1038, 6500, 11050, None);
        let p = build_multivariant_playlist(&[("480p", &low), ("original", &orig)]);
        assert_eq!(
            p,
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n\
             #EXT-X-STREAM-INF:BANDWIDTH=1872000,AVERAGE-BANDWIDTH=1592000,RESOLUTION=854x462,CODECS=\"hvc1.2.4.L90.B0,mp4a.40.2\"\n480p/index.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=11050000,AVERAGE-BANDWIDTH=6500000,RESOLUTION=1920x1038\noriginal/index.m3u8\n"
        );
    }

    #[test]
    fn master_playlist_shape() {
        let m = build_master_playlist(4_160_000, Some((1280, 720)));
        assert!(m.contains("BANDWIDTH=4160000,RESOLUTION=1280x720\nmain/index.m3u8"));
    }
}

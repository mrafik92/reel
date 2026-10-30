//! Browser output: one shared `ffmpeg -c copy` per (channel, audio/video mode) fed straight from
//! the hub over a stdin pipe, writing a sliding-window fMP4 HLS playlist with ~1 s segments.
//! Any number of HLS viewers share that one ffmpeg; it goes away shortly after the last viewer.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use reel_playback::HwAccel;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    ChannelHub, Error, Result,
    ts::{AudioCodec, StreamInfo, VideoCodec},
};

/// What the client can play without re-encoding (codec names as in `ts::*Codec::name`).
#[derive(Debug, Clone)]
pub struct HlsOptions {
    pub video_copy: Vec<String>,
    pub audio_copy: Vec<String>,
}

impl Default for HlsOptions {
    fn default() -> Self {
        Self {
            video_copy: vec!["h264".into()],
            audio_copy: vec!["aac".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoMode {
    Copy,
    Transcode,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    Copy,
    Aac,
    None,
}

fn vname(m: VideoMode) -> &'static str {
    match m {
        VideoMode::Copy => "copy",
        VideoMode::Transcode => "x264",
        VideoMode::None => "novideo",
    }
}

fn aname(m: AudioMode) -> &'static str {
    match m {
        AudioMode::Copy => "copy",
        AudioMode::Aac => "aac",
        AudioMode::None => "noaudio",
    }
}

fn video_copyable(c: VideoCodec, o: &HlsOptions) -> bool {
    if c == VideoCodec::Mpeg2 {
        return false;
    }
    o.video_copy.iter().any(|x| {
        let x = x.to_ascii_lowercase();
        x == c.name()
            || (c == VideoCodec::Hevc && matches!(x.as_str(), "h265" | "hvc1" | "hev1"))
            || (c == VideoCodec::H264 && matches!(x.as_str(), "avc" | "avc1"))
    })
}

/// Copy whatever the client can decode; re-encode only what it cannot.
pub fn decide_modes(info: &StreamInfo, o: &HlsOptions) -> (VideoMode, AudioMode) {
    let v = match info.video {
        None => VideoMode::None,
        Some(c) if video_copyable(c, o) => VideoMode::Copy,
        Some(_) => VideoMode::Transcode,
    };
    let a = match info.audio.first() {
        None => AudioMode::None,
        Some((_, c)) if o.audio_copy.iter().any(|x| x == c.name()) && *c == AudioCodec::Aac => {
            AudioMode::Copy
        }
        Some(_) => AudioMode::Aac,
    };
    (v, a)
}

/// Output height cap for transcoded live video.
const MAX_HEIGHT: &str = "720";

/// `(input-side hw init args, encoder + filter args)` for a video transcode.
pub fn video_encode_args(hw: &HwAccel) -> (Vec<String>, Vec<String>) {
    let s = |x: &str| x.to_string();
    let scale = format!("scale=-2:min({MAX_HEIGHT}\\,ih)");
    let rate = ["-b:v", "4M", "-maxrate", "5M", "-bufsize", "8M", "-g", "50"];
    let force_kf = ["-force_key_frames", "expr:gte(t,n_forced*1)"];
    let (pre, mut post): (Vec<String>, Vec<String>) = match hw {
        HwAccel::Software => (
            vec![],
            [
                "-vf",
                &format!("{scale},format=yuv420p"),
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-sc_threshold",
                "0",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
        ),
        // Full VAAPI pipeline: hardware decode → (de)interlace + scale + 8-bit NV12 on the GPU →
        // hardware H.264 encode. `auto=1` only deinterlaces frames flagged interlaced (typical of
        // IPTV SD/1080i), progressive frames pass through untouched.
        HwAccel::Vaapi { device } => (
            [
                "-init_hw_device",
                &format!("vaapi=va:{device}"),
                "-filter_hw_device",
                "va",
                "-hwaccel",
                "vaapi",
                "-hwaccel_device",
                "va",
                "-hwaccel_output_format",
                "vaapi",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
            [
                "-vf",
                &format!(
                    "deinterlace_vaapi=rate=frame:auto=1,scale_vaapi=w=-2:h=min({MAX_HEIGHT}\\,ih):format=nv12"
                ),
                "-c:v",
                "h264_vaapi",
                "-profile:v",
                "high",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
        ),
        HwAccel::Qsv { device } => (
            [
                "-init_hw_device",
                &format!("vaapi=va:{device}"),
                "-init_hw_device",
                "qsv=qs@va",
                "-filter_hw_device",
                "qs",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
            [
                "-vf",
                &format!("{scale},format=nv12,hwupload=extra_hw_frames=64"),
                "-c:v",
                "h264_qsv",
                "-profile:v",
                "high",
                "-preset",
                "veryfast",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
        ),
        HwAccel::Nvenc => (
            vec![],
            [
                "-vf",
                &format!("{scale},format=yuv420p"),
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-profile:v",
                "high",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
        ),
        HwAccel::VideoToolbox => (
            vec![],
            [
                "-vf",
                &format!("{scale},format=yuv420p"),
                "-c:v",
                "h264_videotoolbox",
                "-profile:v",
                "high",
                "-realtime",
                "1",
            ]
            .iter()
            .map(|x| s(x))
            .collect(),
        ),
    };
    post.extend(rate.iter().map(|x| s(x)));
    post.extend(force_kf.iter().map(|x| s(x)));
    (pre, post)
}

pub fn ffmpeg_args(
    dir: &Path,
    v: VideoMode,
    a: AudioMode,
    hw: &HwAccel,
    hevc_copy: bool,
    seg_secs: u32,
    list_size: u32,
) -> Vec<String> {
    let s = |x: &str| x.to_string();
    let (hw_pre, hw_post) = if v == VideoMode::Transcode {
        video_encode_args(hw)
    } else {
        (vec![], vec![])
    };
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-fflags",
        "+genpts+discardcorrupt",
    ]
    .iter()
    .map(|x| s(x))
    .collect();
    args.extend(hw_pre);
    args.extend(
        [
            "-analyzeduration",
            "1000000",
            "-probesize",
            "1000000",
            "-f",
            "mpegts",
            "-i",
            "pipe:0",
        ]
        .iter()
        .map(|x| s(x)),
    );
    if v != VideoMode::None {
        args.extend([s("-map"), s("0:v:0")]);
    }
    if a != AudioMode::None {
        args.extend([s("-map"), s("0:a:0")]);
    }
    match v {
        VideoMode::Copy => {
            args.extend([s("-c:v"), s("copy")]);
            if hevc_copy {
                // Safari / Chrome / TV browsers only play HEVC-in-fMP4 tagged `hvc1`
                args.extend([s("-tag:v"), s("hvc1")]);
            }
        }
        VideoMode::Transcode => args.extend(hw_post),
        VideoMode::None => {}
    }
    match a {
        AudioMode::Copy => args.extend(
            ["-c:a", "copy", "-bsf:a", "aac_adtstoasc"]
                .iter()
                .map(|x| s(x)),
        ),
        AudioMode::Aac => args.extend(
            ["-c:a", "aac", "-b:a", "128k", "-ac", "2"]
                .iter()
                .map(|x| s(x)),
        ),
        AudioMode::None => {}
    }
    args.extend(
        [
            "-f",
            "hls",
            "-hls_time",
            &seg_secs.to_string(),
            "-hls_init_time",
            "1",
            "-hls_list_size",
            &list_size.to_string(),
            "-hls_flags",
            "delete_segments+independent_segments+omit_endlist",
            "-hls_delete_threshold",
            "2",
            "-hls_segment_type",
            "fmp4",
            "-hls_fmp4_init_filename",
            "init.mp4",
            "-hls_segment_filename",
        ]
        .iter()
        .map(|x| s(x)),
    );
    args.push(dir.join("seg_%05d.m4s").to_string_lossy().into_owned());
    args.push(dir.join("index.m3u8").to_string_lossy().into_owned());
    args
}

pub struct HlsPipe {
    pub key: String,
    pub dir: PathBuf,
    pub video: VideoMode,
    pub audio: AudioMode,
    /// `copy`, or the encoder backend used for the video transcode (`vaapi`, `software`, …).
    pub encoder: &'static str,
    pub channel_id: i64,
    /// Single-connection provider: released as soon as the last viewer leaves.
    limited: bool,
    /// Bytes of MPEG-TS handed to ffmpeg (0 = the input never flowed).
    fed_bytes: AtomicU64,
    /// ffmpeg exit code (`i64::MIN` while running; -1 = killed by a signal).
    exit_code: AtomicI64,
    viewers: AtomicUsize,
    last_used: Mutex<Instant>,
    cancel: CancellationToken,
    alive: AtomicBool,
    stderr_tail: Arc<Mutex<String>>,
}

/// A viewer's hold on an [`HlsPipe`].
pub struct HlsLease {
    pipe: Arc<HlsPipe>,
}

impl HlsLease {
    pub fn pipe(&self) -> &Arc<HlsPipe> {
        &self.pipe
    }
}

impl Drop for HlsLease {
    fn drop(&mut self) {
        if self.pipe.viewers.fetch_sub(1, Relaxed) == 1 {
            *self.pipe.last_used.lock().unwrap() = Instant::now();
        }
    }
}

impl HlsPipe {
    pub fn is_alive(&self) -> bool {
        self.alive.load(Relaxed)
    }
    pub fn viewers(&self) -> usize {
        self.viewers.load(Relaxed)
    }
    /// Tail of ffmpeg's stderr (URLs masked).
    pub fn last_error(&self) -> String {
        crate::net::redact_text(&self.stderr_tail.lock().unwrap())
    }
    fn exit_code(&self) -> Option<i64> {
        Some(self.exit_code.load(Relaxed)).filter(|c| *c != i64::MIN)
    }
}

/// Consecutive hardware-encoder failures after which software is used for a cool-down period.
const HW_FAIL_LIMIT: u32 = 3;
const HW_COOLDOWN: Duration = Duration::from_secs(600);

#[derive(Default)]
struct HwHealth {
    /// Consecutive attempts that failed because of the hardware encoder.
    streak: u32,
    /// Hardware is skipped until then (set after [`HW_FAIL_LIMIT`] failures in a row).
    disabled_until: Option<Instant>,
}

/// Did ffmpeg die because of the hardware encoder/decoder? Only when the input was flowing, the
/// process exited non-zero and stderr names the hardware stack. An upstream that simply closed
/// (no input, or a clean exit) is not the GPU's fault.
pub(crate) fn is_hw_fault(exited: bool, code: Option<i64>, fed_bytes: u64, stderr: &str) -> bool {
    if !exited || fed_bytes == 0 || code.is_none_or(|c| c == 0) {
        return false;
    }
    let t = stderr.to_ascii_lowercase();
    [
        "vaapi",
        "libva",
        "hwaccel",
        "hwupload",
        "hw device",
        "hardware",
        "qsv",
        "mfx",
        "nvenc",
        "cuda",
        "cuvid",
        "videotoolbox",
        "device creation failed",
        "/dev/dri",
        "vaapi_encode",
        "va_",
        "failed to initialise",
        "failed to initialize",
    ]
    .iter()
    .any(|p| t.contains(p))
}

pub struct HlsManager {
    root: PathBuf,
    ffmpeg: String,
    grace: Duration,
    seg_secs: u32,
    list_size: u32,
    hw: Mutex<HwAccel>,
    hw_health: Mutex<HwHealth>,
    pipes: tokio::sync::Mutex<HashMap<String, Arc<HlsPipe>>>,
}

impl HlsManager {
    /// `root` is wiped at startup (stale output of a previous run).
    pub fn new(root: PathBuf, ffmpeg: String, grace: Duration) -> Self {
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(&root);
        Self {
            root,
            ffmpeg,
            grace,
            seg_secs: 1,
            list_size: 6,
            hw: Mutex::new(HwAccel::Software),
            hw_health: Mutex::default(),
            pipes: Default::default(),
        }
    }

    /// Hardware encoder used for video transcodes (set from the startup detection).
    pub fn set_hwaccel(&self, hw: HwAccel) {
        *self.hw.lock().unwrap() = hw;
        *self.hw_health.lock().unwrap() = HwHealth::default();
    }

    /// `(configured encoder, hardware currently disabled after repeated failures)`.
    pub fn hw_status(&self) -> (&'static str, bool) {
        (self.hw.lock().unwrap().name(), self.hw_disabled())
    }

    fn hw_disabled(&self) -> bool {
        let mut h = self.hw_health.lock().unwrap();
        match h.disabled_until {
            Some(t) if Instant::now() < t => true,
            Some(_) => {
                // cool-down over: try the hardware again
                *h = HwHealth::default();
                false
            }
            None => false,
        }
    }

    /// Encoder for the next attempt: the configured hardware unless it is cooling down.
    fn current_hw(&self) -> HwAccel {
        if self.hw_disabled() {
            HwAccel::Software
        } else {
            self.hw.lock().unwrap().clone()
        }
    }

    fn hw_success(&self) {
        *self.hw_health.lock().unwrap() = HwHealth::default();
    }

    /// Record a hardware-attributable failure; returns whether hardware is now disabled.
    fn hw_failure(&self) -> bool {
        let mut h = self.hw_health.lock().unwrap();
        h.streak += 1;
        if h.streak >= HW_FAIL_LIMIT {
            h.disabled_until = Some(Instant::now() + HW_COOLDOWN);
            true
        } else {
            false
        }
    }

    /// Join (or start) the shared HLS output of a channel.
    pub async fn acquire(
        &self,
        hub: &ChannelHub,
        channel_id: i64,
        user_id: Option<i64>,
        opts: &HlsOptions,
    ) -> Result<HlsLease> {
        let sub = hub.subscribe(channel_id, user_id).await?;
        sub.wait_ready(Duration::from_secs(6)).await?;
        let info = sub
            .wait_info(Duration::from_secs(10))
            .await
            .ok_or_else(|| {
                Error::Unavailable("the channel did not deliver a valid stream".into())
            })?;
        drop(sub);
        let (v, a) = decide_modes(&info, opts);
        let hevc_copy = v == VideoMode::Copy && info.video == Some(VideoCodec::Hevc);
        let limited = hub
            .channel_budget(channel_id)
            .await?
            .is_some_and(|(_, m)| m == 1);
        let key = format!("{channel_id}-{}-{}", vname(v), aname(a));
        let mut map = self.pipes.lock().await;
        if let Some(p) = map.get(&key).filter(|p| p.is_alive()).cloned() {
            p.viewers.fetch_add(1, Relaxed);
            *p.last_used.lock().unwrap() = Instant::now();
            return Ok(HlsLease { pipe: p });
        }
        let hw = self.current_hw();
        let mut pipe = self
            .start_pipe(
                hub,
                channel_id,
                user_id,
                &key,
                (v, a),
                &hw,
                hevc_copy,
                limited,
            )
            .await?;
        if v == VideoMode::Transcode && hw != HwAccel::Software {
            if self.wait_ready(&pipe, Duration::from_secs(20)).await {
                self.hw_success();
            } else {
                // let the exit code and the last stderr lines settle
                tokio::time::sleep(Duration::from_millis(150)).await;
                let tail = pipe.last_error();
                let (exited, code, fed) = (
                    !pipe.is_alive(),
                    pipe.exit_code(),
                    pipe.fed_bytes.load(Relaxed),
                );
                if is_hw_fault(exited, code, fed, &tail) {
                    // Per attempt: this start falls back to software, the next one tries the
                    // hardware again; only a streak of failures disables it for a while.
                    let disabled = self.hw_failure();
                    warn!(
                        channel_id,
                        encoder = hw.name(),
                        exit_code = ?code,
                        disabled,
                        stderr = %tail.trim(),
                        "hardware transcode failed; using software for this start"
                    );
                    pipe.cancel.cancel();
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    pipe = self
                        .start_pipe(
                            hub,
                            channel_id,
                            user_id,
                            &key,
                            (v, a),
                            &HwAccel::Software,
                            hevc_copy,
                            limited,
                        )
                        .await?;
                } else {
                    warn!(
                        channel_id,
                        encoder = hw.name(),
                        exited,
                        exit_code = ?code,
                        fed_bytes = fed,
                        stderr = %tail.trim(),
                        "transcoder produced no output; not attributed to the hardware encoder"
                    );
                    if exited {
                        pipe.cancel.cancel();
                        return Err(Error::Unavailable(if fed == 0 {
                            "the channel's stream ended before any video could be produced".into()
                        } else {
                            format!("the transcoder stopped unexpectedly: {}", tail.trim())
                        }));
                    }
                }
            }
        }
        map.insert(key, pipe.clone());
        Ok(HlsLease { pipe })
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_pipe(
        &self,
        hub: &ChannelHub,
        channel_id: i64,
        user_id: Option<i64>,
        key: &str,
        (v, a): (VideoMode, AudioMode),
        hw: &HwAccel,
        hevc_copy: bool,
        limited: bool,
    ) -> Result<Arc<HlsPipe>> {
        let mut sub = hub.subscribe(channel_id, user_id).await?;
        let dir = self.root.join(key);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await?;
        let mut child = tokio::process::Command::new(&self.ffmpeg)
            .args(ffmpeg_args(
                &dir,
                v,
                a,
                hw,
                hevc_copy,
                self.seg_secs,
                self.list_size,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Unavailable(format!("cannot start ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let pipe = Arc::new(HlsPipe {
            key: key.to_string(),
            dir,
            video: v,
            audio: a,
            encoder: if v == VideoMode::Transcode {
                hw.name()
            } else {
                "copy"
            },
            channel_id,
            limited,
            fed_bytes: AtomicU64::new(0),
            exit_code: AtomicI64::new(i64::MIN),
            viewers: AtomicUsize::new(1),
            last_used: Mutex::new(Instant::now()),
            cancel: CancellationToken::new(),
            alive: AtomicBool::new(true),
            stderr_tail: Arc::default(),
        });
        info!(%key, encoder = pipe.encoder, "starting shared HLS output");
        // stderr tail (for diagnostics)
        let tail = pipe.stderr_tail.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = tail.lock().unwrap();
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
                let len = t.len();
                if len > 2048 {
                    let mut cut = len - 2048;
                    while !t.is_char_boundary(cut) {
                        cut += 1;
                    }
                    *t = t[cut..].to_string();
                }
            }
        });
        // hub → ffmpeg stdin
        let p = pipe.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = p.cancel.cancelled() => break,
                    b = sub.next() => match b {
                        Some(b) => {
                            if stdin.write_all(&b).await.is_err() { break }
                            p.fed_bytes.fetch_add(b.len() as u64, Relaxed);
                        }
                        None => break,
                    },
                }
            }
            drop(stdin);
        });
        // supervise ffmpeg
        let p = pipe.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = p.cancel.cancelled() => { let _ = child.kill().await; }
                st = child.wait() => {
                    let code = match &st {
                        Ok(s) => i64::from(s.code().unwrap_or(-1)),
                        Err(_) => -1,
                    };
                    p.exit_code.store(code, Relaxed);
                    debug!(key = %p.key, ?st, "ffmpeg exited");
                }
            }
            p.alive.store(false, Relaxed);
            p.cancel.cancel();
        });
        Ok(pipe)
    }

    /// Wait until the playlist has a segment (or `max` passes).
    pub async fn wait_ready(&self, pipe: &HlsPipe, max: Duration) -> bool {
        let end = Instant::now() + max;
        while Instant::now() < end {
            if !pipe.is_alive() {
                return false;
            }
            if let Ok(t) = tokio::fs::read_to_string(pipe.dir.join("index.m3u8")).await {
                if t.contains("#EXTINF") && pipe.dir.join("init.mp4").exists() {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Tear down every viewer-less output of other channels right now (a single-connection
    /// provider needs its slot back immediately when a user switches channel).
    pub async fn release_unwatched(&self, except_channel: i64) {
        let mut map = self.pipes.lock().await;
        let dead: Vec<String> = map
            .values()
            .filter(|p| p.channel_id != except_channel && p.viewers() == 0)
            .map(|p| p.key.clone())
            .collect();
        for k in dead {
            if let Some(p) = map.remove(&k) {
                p.cancel.cancel();
                let dir = p.dir.clone();
                tokio::spawn(async move {
                    let _ = tokio::fs::remove_dir_all(dir).await;
                });
            }
        }
    }

    /// Remove pipes nobody watches any more (or whose ffmpeg died).
    pub async fn reap(&self) {
        let mut map = self.pipes.lock().await;
        let dead: Vec<String> = map
            .values()
            .filter(|p| {
                !p.is_alive()
                    || (p.viewers() == 0
                        && p.last_used.lock().unwrap().elapsed()
                            >= if p.limited {
                                Duration::ZERO
                            } else {
                                self.grace
                            })
            })
            .map(|p| p.key.clone())
            .collect();
        for k in dead {
            if let Some(p) = map.remove(&k) {
                debug!(key = %k, "closing idle HLS output");
                p.cancel.cancel();
                let dir = p.dir.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = tokio::fs::remove_dir_all(dir).await;
                });
            }
        }
    }

    pub async fn shutdown(&self) {
        let mut map = self.pipes.lock().await;
        for (_, p) in map.drain() {
            p.cancel.cancel();
        }
        let _ = tokio::fs::remove_dir_all(&self.root).await;
    }

    pub async fn active(&self) -> usize {
        self.pipes.lock().await.len()
    }
}

/// Only the files ffmpeg writes may be served.
pub fn valid_hls_file(name: &str) -> bool {
    name == "index.m3u8"
        || name == "init.mp4"
        || (name.len() == 13
            && name.starts_with("seg_")
            && name.ends_with(".m4s")
            && name[4..9].bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts::{AudioCodec, StreamInfo, VideoCodec};

    fn info(v: Option<VideoCodec>, a: Option<AudioCodec>) -> StreamInfo {
        StreamInfo {
            pmt_pid: 0x100,
            video_pid: v.map(|_| 0x101),
            video: v,
            audio: a.map(|c| vec![(0x102, c)]).unwrap_or_default(),
        }
    }

    #[test]
    fn copy_what_the_client_can_play() {
        let o = HlsOptions::default();
        assert_eq!(
            decide_modes(&info(Some(VideoCodec::H264), Some(AudioCodec::Aac)), &o),
            (VideoMode::Copy, AudioMode::Copy)
        );
        assert_eq!(
            decide_modes(&info(Some(VideoCodec::H264), Some(AudioCodec::Mp2)), &o),
            (VideoMode::Copy, AudioMode::Aac)
        );
        assert_eq!(
            decide_modes(&info(Some(VideoCodec::H264), Some(AudioCodec::Ac3)), &o).1,
            AudioMode::Aac
        );
        assert_eq!(
            decide_modes(&info(Some(VideoCodec::Mpeg2), Some(AudioCodec::Mp2)), &o),
            (VideoMode::Transcode, AudioMode::Aac)
        );
        assert_eq!(
            decide_modes(&info(Some(VideoCodec::Hevc), Some(AudioCodec::Aac)), &o).0,
            VideoMode::Transcode
        );
        let hevc_ok = HlsOptions {
            video_copy: vec!["h264".into(), "hevc".into()],
            audio_copy: vec!["aac".into(), "ac3".into()],
        };
        assert_eq!(
            decide_modes(
                &info(Some(VideoCodec::Hevc), Some(AudioCodec::Aac)),
                &hevc_ok
            )
            .0,
            VideoMode::Copy
        );
        // AC-3 is never fMP4-HLS-safe for browsers here: always AAC unless AAC already
        assert_eq!(
            decide_modes(
                &info(Some(VideoCodec::H264), Some(AudioCodec::Ac3)),
                &hevc_ok
            )
            .1,
            AudioMode::Aac
        );
        assert_eq!(
            decide_modes(&info(None, Some(AudioCodec::Aac)), &o).0,
            VideoMode::None
        );
    }

    #[test]
    fn args_are_copy_by_default() {
        let a = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Copy,
            AudioMode::Copy,
            &HwAccel::Software,
            false,
            1,
            6,
        );
        let j = a.join(" ");
        assert!(j.contains("-c:v copy") && j.contains("-c:a copy"));
        assert!(j.contains("-hls_time 1") && j.contains("-hls_segment_type fmp4"));
        assert!(j.ends_with("/tmp/x/index.m3u8"));
        let t = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Transcode,
            AudioMode::Aac,
            &HwAccel::Software,
            false,
            1,
            6,
        )
        .join(" ");
        assert!(t.contains("libx264") && t.contains("-c:a aac"));
    }

    #[test]
    fn hevc_is_copied_for_capable_clients_with_the_hvc1_tag() {
        let hevc_info = StreamInfo {
            pmt_pid: 0x100,
            video_pid: Some(0x100),
            video: Some(VideoCodec::Hevc),
            audio: vec![(0x101, AudioCodec::Mp2)],
        };
        let capable = HlsOptions {
            video_copy: vec!["h264".into(), "hevc".into()],
            audio_copy: vec!["aac".into()],
        };
        assert_eq!(
            decide_modes(&hevc_info, &capable),
            (VideoMode::Copy, AudioMode::Aac)
        );
        let alias = HlsOptions {
            video_copy: vec!["h265".into()],
            ..capable.clone()
        };
        assert_eq!(decide_modes(&hevc_info, &alias).0, VideoMode::Copy);
        // the default profile (H.264 only) still transcodes
        assert_eq!(
            decide_modes(&hevc_info, &HlsOptions::default()).0,
            VideoMode::Transcode
        );
        let args = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Copy,
            AudioMode::Aac,
            &HwAccel::Software,
            true,
            1,
            6,
        )
        .join(" ");
        assert!(
            args.contains("-c:v copy -tag:v hvc1") && args.contains("-c:a aac"),
            "{args}"
        );
        let h264 = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Copy,
            AudioMode::Copy,
            &HwAccel::Software,
            false,
            1,
            6,
        )
        .join(" ");
        assert!(!h264.contains("hvc1"));
    }

    #[test]
    fn hardware_fault_attribution() {
        let vaapi_err =
            "[AVHWDeviceContext] Failed to initialise VAAPI connection: -1 (unknown libva error).";
        // genuine hardware failure: ffmpeg exited non-zero with input flowing and a VAAPI error
        assert!(is_hw_fault(true, Some(1), 50_000, vaapi_err));
        // the upstream closed: nothing was fed to ffmpeg
        assert!(!is_hw_fault(true, Some(1), 0, vaapi_err));
        assert!(!is_hw_fault(true, Some(1), 0, ""));
        // empty stderr is never a hardware verdict
        assert!(!is_hw_fault(true, Some(1), 50_000, ""));
        // still running (no segment yet), or a clean exit
        assert!(!is_hw_fault(false, None, 50_000, vaapi_err));
        assert!(!is_hw_fault(true, Some(0), 50_000, vaapi_err));
        // non-hardware errors
        assert!(!is_hw_fault(
            true,
            Some(1),
            50_000,
            "Invalid data found when processing input"
        ));
        assert!(!is_hw_fault(
            true,
            Some(1),
            50_000,
            "Connection reset by peer"
        ));
    }

    #[test]
    fn hardware_is_disabled_only_after_a_streak_and_recovers() {
        let m = HlsManager::new(
            std::env::temp_dir().join("reel-hls-test-health"),
            "ffmpeg".into(),
            Duration::from_secs(1),
        );
        m.set_hwaccel(HwAccel::Vaapi {
            device: "/dev/dri/renderD128".into(),
        });
        assert_eq!(m.hw_status(), ("vaapi", false));
        assert!(!m.hw_failure());
        assert!(!m.hw_failure());
        assert_eq!(
            m.hw_status(),
            ("vaapi", false),
            "per-attempt: still trying the hardware"
        );
        m.hw_success();
        assert!(
            !m.hw_failure() && !m.hw_failure(),
            "a success resets the streak"
        );
        assert!(m.hw_failure(), "3rd consecutive failure disables");
        assert_eq!(m.hw_status(), ("vaapi", true));
        assert_eq!(m.current_hw(), HwAccel::Software);
        m.hw_health.lock().unwrap().disabled_until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(m.hw_status(), ("vaapi", false), "cool-down over");
        assert!(matches!(m.current_hw(), HwAccel::Vaapi { .. }));
    }

    #[test]
    fn hardware_encoder_args_per_backend() {
        let line = |hw: HwAccel| {
            ffmpeg_args(
                Path::new("/tmp/x"),
                VideoMode::Transcode,
                AudioMode::Aac,
                &hw,
                false,
                1,
                6,
            )
            .join(" ")
        };
        let sw = line(HwAccel::Software);
        assert!(sw.contains("-c:v libx264") && !sw.contains("hw_device"));
        let va = line(HwAccel::Vaapi {
            device: "/dev/dri/renderD128".into(),
        });
        assert!(va.contains("-init_hw_device vaapi=va:/dev/dri/renderD128 -filter_hw_device va"));
        // full pipeline: hw decode, GPU deinterlace + scale, hw encode
        assert!(va.contains("-hwaccel vaapi -hwaccel_device va -hwaccel_output_format vaapi"));
        assert!(
            va.contains(
                "deinterlace_vaapi=rate=frame:auto=1,scale_vaapi=w=-2:h=min(720\\,ih):format=nv12"
            ),
            "{va}"
        );
        assert!(va.contains("-c:v h264_vaapi") && !va.contains("hwupload"));
        assert!(va.find("-hwaccel ").unwrap() < va.find("-i pipe:0").unwrap());
        assert!(
            va.find("-init_hw_device").unwrap() < va.find("-i pipe:0").unwrap(),
            "device init precedes the input"
        );
        let qsv = line(HwAccel::Qsv {
            device: "/dev/dri/renderD128".into(),
        });
        assert!(
            qsv.contains("-init_hw_device qsv=qs@va")
                && qsv.contains("-c:v h264_qsv")
                && qsv.contains("hwupload=extra_hw_frames=64")
        );
        assert!(line(HwAccel::Nvenc).contains("-c:v h264_nvenc"));
        assert!(line(HwAccel::VideoToolbox).contains("-c:v h264_videotoolbox"));
        // copy mode never touches the hardware
        let copy = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Copy,
            AudioMode::Copy,
            &HwAccel::Nvenc,
            false,
            1,
            6,
        )
        .join(" ");
        assert!(!copy.contains("nvenc") && copy.contains("-c:v copy"));
    }

    #[test]
    fn only_ffmpeg_outputs_are_servable() {
        for ok in ["index.m3u8", "init.mp4", "seg_00001.m4s"] {
            assert!(valid_hls_file(ok), "{ok}");
        }
        for bad in [
            "../x",
            "seg_1.m4s",
            "seg_00001.ts",
            "index.m3u8.tmp",
            "",
            "/etc/passwd",
            "seg_0000a.m4s",
        ] {
            assert!(!valid_hls_file(bad), "{bad}");
        }
    }
}

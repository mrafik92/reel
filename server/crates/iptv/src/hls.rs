//! Browser output: one shared `ffmpeg -c copy` per (channel, audio/video mode) fed straight from
//! the hub over a stdin pipe, writing a sliding-window fMP4 HLS playlist with ~1 s segments.
//! Any number of HLS viewers share that one ffmpeg; it goes away shortly after the last viewer.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

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

/// Copy whatever the client can decode; re-encode only what it cannot.
pub fn decide_modes(info: &StreamInfo, o: &HlsOptions) -> (VideoMode, AudioMode) {
    let v = match info.video {
        None => VideoMode::None,
        Some(c) if o.video_copy.iter().any(|x| x == c.name()) && c != VideoCodec::Mpeg2 => {
            VideoMode::Copy
        }
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

pub fn ffmpeg_args(
    dir: &Path,
    v: VideoMode,
    a: AudioMode,
    seg_secs: u32,
    list_size: u32,
) -> Vec<String> {
    let s = |x: &str| x.to_string();
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-fflags",
        "+genpts+discardcorrupt",
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
    .map(|x| s(x))
    .collect();
    if v != VideoMode::None {
        args.extend([s("-map"), s("0:v:0")]);
    }
    if a != AudioMode::None {
        args.extend([s("-map"), s("0:a:0")]);
    }
    match v {
        VideoMode::Copy => args.extend([s("-c:v"), s("copy")]),
        VideoMode::Transcode => args.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "scale=-2:min(720\\,ih)",
                "-g",
                "50",
                "-sc_threshold",
                "0",
                "-force_key_frames",
                "expr:gte(t,n_forced*1)",
            ]
            .iter()
            .map(|x| s(x)),
        ),
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
    pub fn last_error(&self) -> String {
        self.stderr_tail.lock().unwrap().clone()
    }
}

pub struct HlsManager {
    root: PathBuf,
    ffmpeg: String,
    grace: Duration,
    seg_secs: u32,
    list_size: u32,
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
            pipes: Default::default(),
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
        let info = sub
            .wait_info(Duration::from_secs(10))
            .await
            .ok_or_else(|| {
                Error::Unavailable("the channel did not deliver a valid stream".into())
            })?;
        let (v, a) = decide_modes(&info, opts);
        let key = format!("{channel_id}-{}-{}", vname(v), aname(a));
        let mut map = self.pipes.lock().await;
        if let Some(p) = map.get(&key).filter(|p| p.is_alive()).cloned() {
            p.viewers.fetch_add(1, Relaxed);
            *p.last_used.lock().unwrap() = Instant::now();
            return Ok(HlsLease { pipe: p });
        }
        let dir = self.root.join(&key);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await?;
        let mut child = tokio::process::Command::new(&self.ffmpeg)
            .args(ffmpeg_args(&dir, v, a, self.seg_secs, self.list_size))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Unavailable(format!("cannot start ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let pipe = Arc::new(HlsPipe {
            key: key.clone(),
            dir,
            video: v,
            audio: a,
            viewers: AtomicUsize::new(1),
            last_used: Mutex::new(Instant::now()),
            cancel: CancellationToken::new(),
            alive: AtomicBool::new(true),
            stderr_tail: Arc::default(),
        });
        info!(%key, "starting shared HLS output");
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
                    *t = t[len - 2048..].to_string();
                }
            }
        });
        // hub → ffmpeg stdin
        let (p, mut sub) = (pipe.clone(), sub);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = p.cancel.cancelled() => break,
                    b = sub.next() => match b {
                        Some(b) => if stdin.write_all(&b).await.is_err() { break },
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
                st = child.wait() => debug!(key = %p.key, ?st, "ffmpeg exited"),
            }
            p.alive.store(false, Relaxed);
            p.cancel.cancel();
        });
        map.insert(key, pipe.clone());
        Ok(HlsLease { pipe })
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

    /// Remove pipes nobody watches any more (or whose ffmpeg died).
    pub async fn reap(&self) {
        let mut map = self.pipes.lock().await;
        let dead: Vec<String> = map
            .values()
            .filter(|p| {
                !p.is_alive()
                    || (p.viewers() == 0 && p.last_used.lock().unwrap().elapsed() >= self.grace)
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
        let a = ffmpeg_args(Path::new("/tmp/x"), VideoMode::Copy, AudioMode::Copy, 1, 6);
        let j = a.join(" ");
        assert!(j.contains("-c:v copy") && j.contains("-c:a copy"));
        assert!(j.contains("-hls_time 1") && j.contains("-hls_segment_type fmp4"));
        assert!(j.ends_with("/tmp/x/index.m3u8"));
        let t = ffmpeg_args(
            Path::new("/tmp/x"),
            VideoMode::Transcode,
            AudioMode::Aac,
            1,
            6,
        )
        .join(" ");
        assert!(t.contains("libx264") && t.contains("-c:a aac"));
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

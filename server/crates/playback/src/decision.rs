//! Pure decision engine: file + streams + device profile → how to play it.

use crate::model::{FileInfo, Hdr, StreamInfo, StreamKind};
use crate::profile::{DeviceProfile, PlayerKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    DirectPlay,
    Remux,
    AudioTranscode,
    Transcode,
}

impl PlaybackMode {
    /// Value for `playback_sessions.mode` (`direct|remux|transcode`).
    pub fn db_mode(self) -> &'static str {
        match self {
            PlaybackMode::DirectPlay => "direct",
            PlaybackMode::Remux => "remux",
            PlaybackMode::AudioTranscode | PlaybackMode::Transcode => "transcode",
        }
    }
    /// Whether an ffmpeg process is needed.
    pub fn needs_ffmpeg(self) -> bool {
        self != PlaybackMode::DirectPlay
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VideoAction {
    /// File has no usable video stream (audio-only).
    None,
    Copy,
    Encode {
        codec: String,
        width: u32,
        height: u32,
        bitrate_kbps: u32,
        tonemap: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioAction {
    None,
    Copy,
    Encode {
        codec: String,
        channels: u8,
        bitrate_kbps: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubtitleAction {
    None,
    /// Converted to WebVTT and served separately (`/subs/{streamId}.vtt`).
    Sidecar {
        format: String,
    },
    /// Rendered into the video (forces a video transcode).
    BurnIn,
    /// Client renders the embedded track itself (direct play only).
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackDecision {
    pub mode: PlaybackMode,
    pub video: VideoAction,
    pub audio: AudioAction,
    pub subtitle: SubtitleAction,
    pub reasons: Vec<String>,
    /// ffprobe indexes of the streams that are used (None if absent).
    pub video_index: Option<i64>,
    pub audio_index: Option<i64>,
    pub subtitle_index: Option<i64>,
}

pub struct DecisionInput<'a> {
    pub file: &'a FileInfo,
    pub streams: &'a [StreamInfo],
    pub audio_index: Option<i64>,
    pub subtitle_index: Option<i64>,
    pub profile: &'a DeviceProfile,
    /// Per-user bitrate cap (kbit/s) from `user_limits.max_bitrate_kbps`.
    pub max_bitrate_kbps: Option<u32>,
}

/// Normalise ffprobe codec names to the vocabulary used by profiles.
pub fn normalize_codec(c: &str) -> String {
    let c = c.to_ascii_lowercase();
    match c.as_str() {
        "h265" | "hevc" => "hevc",
        "avc" | "avc1" | "h264" => "h264",
        "subrip" | "srt" | "text" => "srt",
        "ssa" | "ass" => "ass",
        "webvtt" | "vtt" => "vtt",
        "hdmv_pgs_subtitle" | "pgssub" | "pgs" => "pgs",
        "dvd_subtitle" | "vobsub" => "vobsub",
        "dvb_subtitle" | "dvbsub" => "dvbsub",
        "dca" | "dts" => "dts",
        "ac-3" | "ac3" => "ac3",
        "e-ac-3" | "eac3" => "eac3",
        other => return other.to_string(),
    }
    .to_string()
}

pub fn is_image_subtitle(codec: &str) -> bool {
    matches!(
        normalize_codec(codec).as_str(),
        "pgs" | "vobsub" | "dvbsub" | "xsub"
    )
}

/// Normalise container from ffprobe `format_name` and/or file extension.
pub fn normalize_container(container: Option<&str>, path: &str) -> Option<String> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if let Some(e) = ext.as_deref() {
        match e {
            "mkv" | "mka" => return Some("mkv".into()),
            "webm" => return Some("webm".into()),
            "mp4" | "m4v" => return Some("mp4".into()),
            "mov" => return Some("mov".into()),
            "ts" | "m2ts" | "mts" => return Some("ts".into()),
            "avi" => return Some("avi".into()),
            _ => {}
        }
    }
    let c = container?.to_ascii_lowercase();
    for tok in c.split(',') {
        let t = match tok.trim() {
            "matroska" => "mkv",
            "mpegts" => "ts",
            "mp4" | "m4v" => "mp4",
            other => other,
        };
        if matches!(t, "mkv" | "webm" | "mp4" | "mov" | "ts" | "avi") {
            return Some(t.to_string());
        }
    }
    None
}

fn profile_rank(codec: &str, profile: &str) -> Option<u32> {
    let p = profile.to_ascii_lowercase();
    let p = p.trim();
    match codec {
        "h264" => Some(match p {
            "baseline" | "constrained baseline" => 0,
            "main" | "extended" => 1,
            "high" | "constrained high" | "progressive high" => 2,
            "high 10" | "high 10 intra" => 3,
            "high 4:2:2" | "high 4:2:2 intra" => 4,
            "high 4:4:4 predictive" | "high 4:4:4" | "high 4:4:4 intra" => 5,
            _ => return None,
        }),
        "hevc" => Some(match p {
            "main" | "main still picture" => 1,
            "main 10" => 2,
            "main 4:4:4"
            | "main 4:4:4 10"
            | "main 4:4:4 12"
            | "main 12"
            | "main 10 still picture" => 3,
            "rext" => 3,
            _ => return None,
        }),
        _ => None,
    }
}

fn even(v: f64) -> u32 {
    let n = v.round() as u32;
    (n & !1).max(2)
}

/// Fit (w,h) into (mw,mh) preserving aspect ratio; never upscales.
fn fit(w: u32, h: u32, mw: u32, mh: u32) -> (u32, u32) {
    if w <= mw && h <= mh {
        return (w & !1, h & !1);
    }
    let s = (mw as f64 / w as f64).min(mh as f64 / h as f64);
    (even(w as f64 * s), even(h as f64 * s))
}

fn ladder_height(video_kbps: u32) -> u32 {
    match video_kbps {
        16000.. => 2160,
        6000.. => 1080,
        3000.. => 720,
        1200.. => 480,
        _ => 360,
    }
}

fn bitrate_for_height(h: u32) -> u32 {
    match h {
        1800.. => 20000,
        900.. => 8000,
        600.. => 4000,
        400.. => 1800,
        _ => 800,
    }
}

fn pick_audio<'a>(
    streams: &'a [StreamInfo],
    want: Option<i64>,
    reasons: &mut Vec<String>,
) -> Option<&'a StreamInfo> {
    let audios: Vec<&StreamInfo> = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .collect();
    if let Some(i) = want {
        if let Some(s) = audios.iter().find(|s| s.idx == i) {
            return Some(s);
        }
        reasons.push(format!(
            "requested audio stream {i} not found; using default"
        ));
    }
    audios
        .iter()
        .find(|s| s.default)
        .or(audios.first())
        .copied()
}

/// Decide how to deliver `input.file` to `input.profile`.
pub fn decide(input: &DecisionInput<'_>) -> PlaybackDecision {
    let profile = input.profile;
    let mut reasons: Vec<String> = Vec::new();

    let video = input.streams.iter().find(|s| {
        s.kind == StreamKind::Video
            && s.codec.as_deref() != Some("mjpeg")
            && s.codec.as_deref() != Some("png")
    });
    let audio = pick_audio(input.streams, input.audio_index, &mut reasons);
    let first_audio = input
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Audio && s.default)
        .or_else(|| input.streams.iter().find(|s| s.kind == StreamKind::Audio));
    let sub = input.subtitle_index.and_then(|i| {
        input
            .streams
            .iter()
            .find(|s| s.kind == StreamKind::Subtitle && s.idx == i)
    });

    let cap = match (profile.max_bitrate_kbps, input.max_bitrate_kbps) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let file_kbps = input.file.bitrate.map(|b| (b / 1000).max(0) as u32);
    let over_cap = matches!((cap, file_kbps), (Some(c), Some(f)) if f > c);
    if over_cap {
        reasons.push(format!(
            "bitrate {} kbps exceeds cap {} kbps",
            file_kbps.unwrap_or(0),
            cap.unwrap_or(0)
        ));
    }

    // ---- container ----
    let container = normalize_container(input.file.container.as_deref(), &input.file.path);
    let container_ok = container
        .as_deref()
        .is_some_and(|c| profile.supports_container(c));
    if !container_ok {
        reasons.push(format!(
            "container {} not supported by {}",
            container.as_deref().unwrap_or("unknown"),
            profile.name
        ));
    }

    // ---- subtitles ----
    let mut subtitle = SubtitleAction::None;
    if let Some(s) = sub {
        let codec = normalize_codec(s.codec.as_deref().unwrap_or(""));
        if is_image_subtitle(&codec) {
            // Image subs can only be rendered by the client in a native direct play.
            if profile.subtitles.image.contains(&codec) {
                subtitle = SubtitleAction::Native; // downgraded below if not direct play
            } else {
                subtitle = SubtitleAction::BurnIn;
                reasons.push(format!(
                    "image subtitle ({codec}) not renderable by client: burn-in"
                ));
            }
        } else if s.external_path.is_some() || !profile.subtitles.text.is_empty() {
            if profile.subtitles.text.contains(&codec) && s.external_path.is_none() {
                subtitle = SubtitleAction::Native; // downgraded below if not direct play
            } else {
                subtitle = SubtitleAction::Sidecar {
                    format: "vtt".into(),
                };
            }
        } else {
            subtitle = SubtitleAction::BurnIn;
            reasons.push("client renders no text subtitles: burn-in".into());
        }
    }
    let burn_in = subtitle == SubtitleAction::BurnIn;

    // ---- video ----
    let mut video_copy_ok = true;
    let mut tonemap_needed = false;
    let mut vreasons: Vec<String> = Vec::new();
    let mut target_dims: Option<(u32, u32)> = None;
    if let Some(v) = video {
        let codec = normalize_codec(v.codec.as_deref().unwrap_or(""));
        match profile.video_support(&codec) {
            None => {
                video_copy_ok = false;
                vreasons.push(format!("video codec {codec} not supported"));
            }
            Some(sup) => {
                if let (Some(max), Some(p)) = (sup.max_profile.as_deref(), v.profile.as_deref()) {
                    if let (Some(mr), Some(pr)) =
                        (profile_rank(&codec, max), profile_rank(&codec, p))
                    {
                        if pr > mr {
                            video_copy_ok = false;
                            vreasons.push(format!("{codec} profile '{p}' exceeds '{max}'"));
                        }
                    }
                }
                if let (Some(max), Some(l)) = (sup.max_level, v.level) {
                    if l > max as i64 {
                        video_copy_ok = false;
                        vreasons.push(format!("{codec} level {l} exceeds {max}"));
                    }
                }
                let depth = v.bit_depth.unwrap_or(8);
                if depth > sup.max_bit_depth as i64 {
                    video_copy_ok = false;
                    vreasons.push(format!("bit depth {depth} exceeds {}", sup.max_bit_depth));
                }
                if v.hdr != Hdr::None && !sup.hdr.contains(&v.hdr) {
                    video_copy_ok = false;
                    tonemap_needed = true;
                    vreasons.push(format!(
                        "{:?} HDR not supported: tone-mapping to SDR",
                        v.hdr
                    ));
                }
            }
        }
        let (w, h) = (
            v.width.unwrap_or(0).max(0) as u32,
            v.height.unwrap_or(0).max(0) as u32,
        );
        if w > 0 && h > 0 {
            let (fw, fh) = fit(w, h, profile.max_width, profile.max_height);
            if (fw, fh) != (w & !1, h & !1) {
                video_copy_ok = false;
                vreasons.push(format!(
                    "resolution {w}x{h} exceeds client max {}x{}",
                    profile.max_width, profile.max_height
                ));
            }
            target_dims = Some((fw, fh));
        }
    }
    reasons.extend(vreasons);
    if burn_in {
        video_copy_ok = false;
    }
    if over_cap {
        video_copy_ok = false;
    }

    // ---- audio ----
    let mut audio_copy_ok = true;
    if let Some(a) = audio {
        let codec = normalize_codec(a.codec.as_deref().unwrap_or(""));
        match profile.audio_support(&codec) {
            None => {
                audio_copy_ok = false;
                reasons.push(format!("audio codec {codec} not supported"));
            }
            Some(sup) => {
                let ch = a.channels.unwrap_or(2);
                // MSE browsers downmix multichannel AAC/Opus themselves, so for them
                // `max_channels` is not a hard limit (only a sanity cap of 8).
                let limit = if profile.player == PlayerKind::Mse
                    && matches!(codec.as_str(), "aac" | "opus")
                {
                    sup.max_channels.max(8)
                } else {
                    sup.max_channels
                };
                if ch > limit as i64 {
                    audio_copy_ok = false;
                    reasons.push(format!("{ch} audio channels exceed client max {}", limit));
                }
            }
        }
    }

    // ---- mode ----
    let audio_track_ok = match (audio, first_audio) {
        (Some(a), Some(f)) => a.idx == f.idx || profile.player == PlayerKind::Native,
        _ => true,
    };
    let mode = if !video_copy_ok {
        PlaybackMode::Transcode
    } else if !audio_copy_ok {
        PlaybackMode::AudioTranscode
    } else if container_ok && audio_track_ok {
        PlaybackMode::DirectPlay
    } else {
        if container_ok && !audio_track_ok {
            reasons.push("selected audio track is not the default: remux to pick it".into());
        }
        PlaybackMode::Remux
    };

    // Embedded rendering only makes sense in direct play.
    if mode != PlaybackMode::DirectPlay && subtitle == SubtitleAction::Native {
        let is_image = sub.is_some_and(|s| is_image_subtitle(s.codec.as_deref().unwrap_or("")));
        if is_image {
            subtitle = SubtitleAction::BurnIn;
            reasons.push("image subtitles cannot be carried in HLS: burn-in".into());
        } else {
            subtitle = SubtitleAction::Sidecar {
                format: "vtt".into(),
            };
        }
    }
    // Burn-in discovered late (above) must force transcode.
    let mode = if subtitle == SubtitleAction::BurnIn {
        PlaybackMode::Transcode
    } else {
        mode
    };

    // ---- actions ----
    let video_action = match video {
        None => VideoAction::None,
        Some(_) if mode != PlaybackMode::Transcode => VideoAction::Copy,
        Some(v) => {
            let (sw, sh) = target_dims.unwrap_or((1920, 1080));
            let audio_budget = if cap.is_some() { 192 } else { 0 };
            let (mut tw, mut th) = (sw, sh);
            let mut bitrate = bitrate_for_height(sh);
            if let Some(c) = cap {
                let budget = ((c as f64 * 0.95) as u32)
                    .saturating_sub(audio_budget)
                    .max(300);
                let lh = ladder_height(budget);
                if lh < sh {
                    let scale = lh as f64 / sh as f64;
                    th = even(sh as f64 * scale);
                    tw = even(sw as f64 * scale);
                    reasons.push(format!("downscaling {sw}x{sh} to {tw}x{th} to fit cap"));
                }
                bitrate = bitrate.min(budget).min(bitrate_for_height(th)).max(300);
            } else if let Some(f) = file_kbps {
                bitrate = bitrate.min(f.max(1000));
            }
            let codec = if profile.video_support("h264").is_some() {
                "h264".to_string()
            } else {
                profile
                    .video
                    .first()
                    .map(|v| v.codec.clone())
                    .unwrap_or_else(|| "h264".into())
            };
            let enc_hdr_ok = profile
                .video_support(&codec)
                .is_some_and(|s| s.hdr.contains(&v.hdr));
            let tonemap = v.hdr != Hdr::None && !enc_hdr_ok;
            let _ = tonemap_needed;
            VideoAction::Encode {
                codec,
                width: tw,
                height: th,
                bitrate_kbps: bitrate,
                tonemap,
            }
        }
    };

    let audio_action = match audio {
        None => AudioAction::None,
        Some(_) if matches!(mode, PlaybackMode::DirectPlay | PlaybackMode::Remux) => {
            AudioAction::Copy
        }
        Some(_) if audio_copy_ok => AudioAction::Copy,
        Some(a) => {
            let src_ch = a.channels.unwrap_or(2).clamp(1, 8) as u8;
            let src_codec = normalize_codec(a.codec.as_deref().unwrap_or(""));
            let eac3_ok = profile
                .audio_support("eac3")
                .is_some_and(|s| s.max_channels >= 6);
            let codec = if src_ch > 2
                && eac3_ok
                && matches!(src_codec.as_str(), "ac3" | "eac3" | "dts" | "truehd")
            {
                "eac3"
            } else {
                "aac"
            };
            let max = profile
                .audio_support(codec)
                .map(|s| s.max_channels)
                .unwrap_or(2);
            let channels = src_ch.min(max).min(6);
            let bitrate_kbps = match (codec, channels) {
                ("eac3", 0..=2) => 192,
                ("eac3", _) => 640,
                (_, 0..=2) => 160,
                _ => 384,
            };
            AudioAction::Encode {
                codec: codec.into(),
                channels,
                bitrate_kbps,
            }
        }
    };

    if mode == PlaybackMode::DirectPlay {
        reasons.push("client supports container and all selected streams".into());
    }

    PlaybackDecision {
        mode,
        video: video_action,
        audio: audio_action,
        subtitle,
        reasons,
        video_index: video.map(|v| v.idx),
        audio_index: audio.map(|a| a.idx),
        subtitle_index: sub.map(|s| s.idx),
    }
}

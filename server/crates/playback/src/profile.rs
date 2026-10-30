//! Device profiles: what a client can decode.

use crate::model::Hdr;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayerKind {
    /// Browser Media Source Extensions (hls.js): cannot switch embedded tracks of a direct-played file.
    Mse,
    /// Native pipeline (`<video>` on TV, AVPlay, ExoPlayer): handles track selection itself.
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VideoCodecSupport {
    /// Normalised codec name: h264, hevc, vp9, av1, mpeg2video …
    pub codec: String,
    /// Highest profile (e.g. "high", "main 10"). `None` = any.
    #[serde(default)]
    pub max_profile: Option<String>,
    /// Highest level in ffprobe notation (H.264 4.1 = 41, HEVC 5.1 = 153). `None` = any.
    #[serde(default)]
    pub max_level: Option<u32>,
    #[serde(default = "default_bit_depth")]
    pub max_bit_depth: u8,
    /// HDR formats decodable *and displayed* as HDR.
    #[serde(default)]
    pub hdr: Vec<Hdr>,
}

fn default_bit_depth() -> u8 {
    8
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioCodecSupport {
    pub codec: String,
    pub max_channels: u8,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
pub struct SubtitleSupport {
    /// Text formats the client can render natively: vtt, srt, ass.
    #[serde(default)]
    pub text: Vec<String>,
    /// Image formats the client can render natively: pgs, vobsub. Empty ⇒ burn-in needed.
    #[serde(default)]
    pub image: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceProfile {
    pub name: String,
    pub player: PlayerKind,
    /// Containers playable directly: mp4, mkv, webm, ts, mov, avi.
    pub containers: Vec<String>,
    pub video: Vec<VideoCodecSupport>,
    pub audio: Vec<AudioCodecSupport>,
    #[serde(default)]
    pub subtitles: SubtitleSupport,
    pub max_width: u32,
    pub max_height: u32,
    /// Client-side bitrate cap in kbit/s.
    #[serde(default)]
    pub max_bitrate_kbps: Option<u32>,
}

impl DeviceProfile {
    pub fn video_support(&self, codec: &str) -> Option<&VideoCodecSupport> {
        self.video.iter().find(|v| v.codec == codec)
    }
    pub fn audio_support(&self, codec: &str) -> Option<&AudioCodecSupport> {
        self.audio.iter().find(|a| a.codec == codec)
    }
    pub fn supports_container(&self, c: &str) -> bool {
        self.containers.iter().any(|x| x == c)
    }

    /// Look up a built-in preset by name.
    pub fn preset(name: &str) -> Option<DeviceProfile> {
        match name {
            "web_chrome" => Some(web_chrome()),
            "web_safari" => Some(web_safari()),
            "webos" => Some(webos()),
            "tizen" => Some(tizen()),
            "android_exoplayer" => Some(android_exoplayer()),
            _ => None,
        }
    }

    pub fn preset_names() -> &'static [&'static str] {
        &[
            "web_chrome",
            "web_safari",
            "webos",
            "tizen",
            "android_exoplayer",
        ]
    }
}

fn v(codec: &str, profile: &str, level: u32, depth: u8, hdr: &[Hdr]) -> VideoCodecSupport {
    VideoCodecSupport {
        codec: codec.into(),
        max_profile: if profile.is_empty() {
            None
        } else {
            Some(profile.into())
        },
        max_level: if level == 0 { None } else { Some(level) },
        max_bit_depth: depth,
        hdr: hdr.to_vec(),
    }
}

fn a(codec: &str, ch: u8) -> AudioCodecSupport {
    AudioCodecSupport {
        codec: codec.into(),
        max_channels: ch,
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Chrome/Firefox/Edge via hls.js (MSE).
pub fn web_chrome() -> DeviceProfile {
    DeviceProfile {
        name: "web_chrome".into(),
        player: PlayerKind::Mse,
        containers: strs(&["mp4", "webm"]),
        video: vec![
            v("h264", "high", 51, 8, &[]),
            v("vp9", "", 0, 10, &[Hdr::Hdr10, Hdr::Hlg]),
            v("av1", "", 0, 10, &[Hdr::Hdr10, Hdr::Hlg]),
        ],
        audio: vec![
            a("aac", 8),
            a("mp3", 2),
            a("opus", 8),
            a("flac", 2),
            a("vorbis", 2),
        ],
        subtitles: SubtitleSupport {
            text: strs(&["vtt"]),
            image: vec![],
        },
        max_width: 3840,
        max_height: 2160,
        max_bitrate_kbps: None,
    }
}

/// Safari / iOS native HLS + MP4.
pub fn web_safari() -> DeviceProfile {
    DeviceProfile {
        name: "web_safari".into(),
        player: PlayerKind::Native,
        containers: strs(&["mp4", "mov"]),
        video: vec![
            v("h264", "high", 52, 8, &[]),
            v("hevc", "main 10", 153, 10, &[Hdr::Hdr10, Hdr::Hlg, Hdr::Dv]),
        ],
        audio: vec![
            a("aac", 8),
            a("ac3", 6),
            a("eac3", 6),
            a("mp3", 2),
            a("alac", 6),
            a("flac", 6),
        ],
        subtitles: SubtitleSupport {
            text: strs(&["vtt"]),
            image: vec![],
        },
        max_width: 3840,
        max_height: 2160,
        max_bitrate_kbps: None,
    }
}

/// LG webOS `<video>` (native pipeline).
pub fn webos() -> DeviceProfile {
    DeviceProfile {
        name: "webos".into(),
        player: PlayerKind::Native,
        containers: strs(&["mp4", "mkv", "ts", "mov"]),
        video: vec![
            v("h264", "high", 51, 8, &[]),
            v("hevc", "main 10", 153, 10, &[Hdr::Hdr10, Hdr::Hlg, Hdr::Dv]),
            v("vp9", "", 0, 10, &[Hdr::Hdr10]),
            v("av1", "", 0, 10, &[Hdr::Hdr10]),
        ],
        audio: vec![
            a("aac", 6),
            a("ac3", 6),
            a("eac3", 8),
            a("mp3", 2),
            a("opus", 2),
            a("flac", 6),
        ],
        subtitles: SubtitleSupport {
            text: strs(&["vtt"]),
            image: vec![],
        },
        max_width: 3840,
        max_height: 2160,
        max_bitrate_kbps: None,
    }
}

/// Samsung Tizen (AVPlay).
pub fn tizen() -> DeviceProfile {
    DeviceProfile {
        name: "tizen".into(),
        player: PlayerKind::Native,
        containers: strs(&["mp4", "mkv", "ts"]),
        video: vec![
            v("h264", "high", 51, 8, &[]),
            v("hevc", "main 10", 153, 10, &[Hdr::Hdr10, Hdr::Hlg]),
            v("vp9", "", 0, 10, &[Hdr::Hdr10]),
            v("av1", "", 0, 10, &[Hdr::Hdr10]),
        ],
        audio: vec![
            a("aac", 6),
            a("ac3", 6),
            a("eac3", 8),
            a("mp3", 2),
            a("opus", 2),
            a("flac", 6),
        ],
        subtitles: SubtitleSupport {
            text: strs(&["vtt", "srt"]),
            image: vec![],
        },
        max_width: 3840,
        max_height: 2160,
        max_bitrate_kbps: None,
    }
}

/// Android / Android TV Media3 ExoPlayer.
pub fn android_exoplayer() -> DeviceProfile {
    DeviceProfile {
        name: "android_exoplayer".into(),
        player: PlayerKind::Native,
        containers: strs(&["mp4", "mkv", "webm", "ts", "mov"]),
        video: vec![
            v("h264", "high", 52, 8, &[]),
            v("hevc", "main 10", 153, 10, &[Hdr::Hdr10, Hdr::Hlg, Hdr::Dv]),
            v("vp9", "", 0, 10, &[Hdr::Hdr10, Hdr::Hlg]),
            v("av1", "", 0, 10, &[Hdr::Hdr10]),
        ],
        audio: vec![
            a("aac", 6),
            a("ac3", 6),
            a("eac3", 8),
            a("mp3", 2),
            a("opus", 6),
            a("flac", 8),
            a("vorbis", 2),
        ],
        subtitles: SubtitleSupport {
            text: strs(&["vtt", "srt", "ass"]),
            image: strs(&["pgs", "vobsub"]),
        },
        max_width: 3840,
        max_height: 2160,
        max_bitrate_kbps: None,
    }
}

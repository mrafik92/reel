//! ffprobe invocation and mapping to DB-ready structures.

use std::{path::Path, process::Stdio, time::Duration};

use serde::Deserialize;
use tokio::process::Command;

/// Errors from running/parsing ffprobe.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("failed to spawn ffprobe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffprobe timed out after {0:?}")]
    Timeout(Duration),
    #[error("ffprobe failed ({status}): {stderr}")]
    Failed { status: String, stderr: String },
    #[error("invalid ffprobe output: {0}")]
    Parse(#[from] serde_json::Error),
}

/// HDR classification (matches `streams.hdr` CHECK values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hdr {
    None,
    Hdr10,
    Hlg,
    Dv,
}

impl Hdr {
    pub fn as_str(self) -> &'static str {
        match self {
            Hdr::None => "none",
            Hdr::Hdr10 => "hdr10",
            Hdr::Hlg => "hlg",
            Hdr::Dv => "dv",
        }
    }
}

/// Stream kind (matches `streams.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Video => "video",
            StreamKind::Audio => "audio",
            StreamKind::Subtitle => "subtitle",
        }
    }
}

/// One stream, ready for insertion into `streams`.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub idx: i64,
    pub kind: StreamKind,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub hdr: Hdr,
    pub bit_depth: Option<i64>,
    pub channels: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
}

/// Parsed ffprobe result.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub container: Option<String>,
    pub duration_s: Option<f64>,
    pub bitrate: Option<i64>,
    pub streams: Vec<StreamInfo>,
    /// Raw ffprobe JSON, stored in `media_files.probe_json`.
    pub raw: String,
}

#[derive(Deserialize, Default)]
struct RawProbe {
    #[serde(default)]
    streams: Vec<RawStream>,
    #[serde(default)]
    format: RawFormat,
}

#[derive(Deserialize, Default)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawStream {
    index: Option<i64>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    level: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    pix_fmt: Option<String>,
    color_transfer: Option<String>,
    bits_per_raw_sample: Option<String>,
    bits_per_sample: Option<i64>,
    channels: Option<i64>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    disposition: std::collections::HashMap<String, i64>,
    #[serde(default)]
    side_data_list: Vec<std::collections::HashMap<String, serde_json::Value>>,
}

fn tag<'a>(s: &'a RawStream, key: &str) -> Option<&'a str> {
    s.tags
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

fn bit_depth(s: &RawStream, kind: StreamKind) -> Option<i64> {
    if let Some(n) = s
        .bits_per_raw_sample
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n > 0)
    {
        return Some(n);
    }
    match kind {
        StreamKind::Video => {
            let p = s.pix_fmt.as_deref()?;
            Some(
                if p.contains("p16") || p.contains("16le") || p.contains("16be") {
                    16
                } else if p.contains("12") {
                    12
                } else if p.contains("10") {
                    10
                } else {
                    8
                },
            )
        }
        StreamKind::Audio => s.bits_per_sample.filter(|n| *n > 0),
        StreamKind::Subtitle => None,
    }
}

fn detect_hdr(s: &RawStream) -> Hdr {
    let dovi = s.side_data_list.iter().any(|d| {
        d.get("side_data_type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| {
                let t = t.to_lowercase();
                t.contains("dovi") || t.contains("dolby vision")
            })
    }) || matches!(
        s.codec_tag_string.as_deref(),
        Some("dvh1" | "dvhe" | "dav1" | "dvav" | "dva1")
    );
    if dovi {
        return Hdr::Dv;
    }
    match s.color_transfer.as_deref() {
        Some("smpte2084") => Hdr::Hdr10,
        Some("arib-std-b67") => Hdr::Hlg,
        _ => Hdr::None,
    }
}

/// Map ffprobe's `format_name` (comma list) plus file extension to a short container name.
pub fn container_name(format_name: &str, ext: Option<&str>) -> String {
    let names: Vec<&str> = format_name.split(',').collect();
    if let Some(e) = ext {
        let e = e.to_lowercase();
        if names.contains(&e.as_str()) {
            return e;
        }
        if names.contains(&"matroska") && (e == "mkv" || e == "webm") {
            return e;
        }
        if names.contains(&"mov") && matches!(e.as_str(), "mp4" | "m4v" | "mov") {
            return e;
        }
    }
    match names.first() {
        Some(&"matroska") => "mkv".into(),
        Some(n) => (*n).into(),
        None => String::new(),
    }
}

/// Parse ffprobe JSON output.
pub fn parse_probe(json: &str, ext: Option<&str>) -> Result<ProbeResult, ProbeError> {
    let raw: RawProbe = serde_json::from_str(json)?;
    let mut streams = Vec::new();
    for (n, s) in raw.streams.iter().enumerate() {
        let kind = match s.codec_type.as_deref() {
            Some("video") => StreamKind::Video,
            Some("audio") => StreamKind::Audio,
            Some("subtitle") => StreamKind::Subtitle,
            _ => continue,
        };
        let disp = |k: &str| s.disposition.get(k).copied().unwrap_or(0) != 0;
        if kind == StreamKind::Video && disp("attached_pic") {
            continue;
        }
        let video = kind == StreamKind::Video;
        streams.push(StreamInfo {
            idx: s.index.unwrap_or(n as i64),
            kind,
            codec: s.codec_name.clone(),
            profile: s.profile.clone().filter(|p| !p.is_empty()),
            level: s.level.filter(|l| *l > 0),
            width: s.width.filter(|_| video),
            height: s.height.filter(|_| video),
            hdr: if video { detect_hdr(s) } else { Hdr::None },
            bit_depth: bit_depth(s, kind),
            channels: s.channels.filter(|_| kind == StreamKind::Audio),
            language: tag(s, "language")
                .filter(|l| !l.eq_ignore_ascii_case("und"))
                .map(str::to_string),
            title: tag(s, "title").map(str::to_string),
            default: disp("default"),
            forced: disp("forced"),
        });
    }
    Ok(ProbeResult {
        container: raw
            .format
            .format_name
            .as_deref()
            .map(|n| container_name(n, ext)),
        duration_s: raw.format.duration.and_then(|d| d.parse().ok()),
        bitrate: raw.format.bit_rate.and_then(|b| b.parse().ok()),
        streams,
        raw: json.to_string(),
    })
}

/// Run ffprobe on `path` with a timeout.
pub async fn probe(
    ffprobe: &str,
    path: &Path,
    timeout: Duration,
) -> Result<ProbeResult, ProbeError> {
    let mut cmd = Command::new(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ])
    .arg(path)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| ProbeError::Timeout(timeout))??;
    if !out.status.success() {
        return Err(ProbeError::Failed {
            status: out.status.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let json = String::from_utf8_lossy(&out.stdout);
    parse_probe(&json, path.extension().and_then(|e| e.to_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "streams": [
        {"index":0,"codec_type":"video","codec_name":"hevc","profile":"Main 10","level":153,
         "width":3840,"height":2160,"pix_fmt":"yuv420p10le","color_transfer":"smpte2084",
         "disposition":{"default":1,"forced":0}},
        {"index":1,"codec_type":"video","codec_name":"h264","width":3840,"height":2160,"pix_fmt":"yuv420p",
         "side_data_list":[{"side_data_type":"DOVI configuration record","dv_profile":8}]},
        {"index":2,"codec_type":"video","codec_name":"hevc","pix_fmt":"yuv420p10le","color_transfer":"arib-std-b67"},
        {"index":3,"codec_type":"audio","codec_name":"eac3","channels":6,"bits_per_sample":0,
         "tags":{"language":"eng","title":"Surround"},"disposition":{"default":1}},
        {"index":4,"codec_type":"audio","codec_name":"aac","channels":2,"tags":{"language":"und"}},
        {"index":5,"codec_type":"subtitle","codec_name":"subrip","tags":{"language":"spa"},
         "disposition":{"forced":1}},
        {"index":6,"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}},
        {"index":7,"codec_type":"attachment","codec_name":"ttf"}
      ],
      "format": {"format_name":"matroska,webm","duration":"123.456000","bit_rate":"8000000"}
    }"#;

    #[test]
    fn maps_streams() {
        let p = parse_probe(SAMPLE, Some("mkv")).unwrap();
        assert_eq!(p.container.as_deref(), Some("mkv"));
        assert_eq!(p.duration_s, Some(123.456));
        assert_eq!(p.bitrate, Some(8_000_000));
        assert_eq!(p.streams.len(), 6);
        let v = &p.streams[0];
        assert_eq!(v.kind, StreamKind::Video);
        assert_eq!(v.hdr, Hdr::Hdr10);
        assert_eq!(v.bit_depth, Some(10));
        assert_eq!(v.profile.as_deref(), Some("Main 10"));
        assert_eq!(v.level, Some(153));
        assert_eq!((v.width, v.height), (Some(3840), Some(2160)));
        assert!(v.default && !v.forced);
        assert_eq!(p.streams[1].hdr, Hdr::Dv);
        assert_eq!(p.streams[1].bit_depth, Some(8));
        assert_eq!(p.streams[2].hdr, Hdr::Hlg);
        let a = &p.streams[3];
        assert_eq!(a.channels, Some(6));
        assert_eq!(a.language.as_deref(), Some("eng"));
        assert_eq!(a.title.as_deref(), Some("Surround"));
        assert_eq!(a.bit_depth, None);
        assert_eq!(p.streams[4].language, None);
        let s = &p.streams[5];
        assert_eq!(s.kind, StreamKind::Subtitle);
        assert!(s.forced);
    }

    #[test]
    fn container_names() {
        assert_eq!(container_name("matroska,webm", Some("webm")), "webm");
        assert_eq!(container_name("matroska,webm", None), "mkv");
        assert_eq!(
            container_name("mov,mp4,m4a,3gp,3g2,mj2", Some("mp4")),
            "mp4"
        );
        assert_eq!(container_name("avi", Some("avi")), "avi");
        assert_eq!(container_name("mpegts", Some("ts")), "mpegts");
    }

    #[test]
    fn bad_json_errors() {
        assert!(parse_probe("not json", None).is_err());
    }

    #[tokio::test]
    async fn missing_binary_errors() {
        let r = probe(
            "definitely-not-ffprobe-xyz",
            Path::new("/nope"),
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(r, Err(ProbeError::Spawn(_))));
    }
}

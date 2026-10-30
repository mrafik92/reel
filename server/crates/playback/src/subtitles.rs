//! Text subtitle extraction/conversion to WebVTT (cached on disk).

use crate::error::PlaybackError;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Where to read the subtitle from.
#[derive(Debug, Clone)]
pub enum SubtitleSource {
    /// Embedded stream: `path` is the media file, `stream_index` the ffprobe stream index.
    Embedded { path: String, stream_index: i64 },
    /// Sidecar file (srt/ass/ssa/vtt).
    External { path: String },
}

pub fn extract_args(src: &SubtitleSource, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let SubtitleSource::Embedded { path, .. } = src {
        if crate::ffmpeg::is_remote(path) {
            // provider URL: player-like agent, network protocols only
            a.extend(
                [
                    "-user_agent",
                    crate::ffmpeg::REMOTE_UA,
                    "-protocol_whitelist",
                    "http,https,tcp,tls,crypto",
                ]
                .iter()
                .map(|s| s.to_string()),
            );
        }
    }
    match src {
        SubtitleSource::Embedded { path, stream_index } => {
            a.extend([
                "-i".into(),
                path.clone(),
                "-map".into(),
                format!("0:{stream_index}"),
            ]);
        }
        SubtitleSource::External { path } => {
            a.extend(["-i".into(), path.clone(), "-map".into(), "0:s:0".into()]);
        }
    }
    a.extend([
        "-c:s".into(),
        "webvtt".into(),
        "-f".into(),
        "webvtt".into(),
        out.to_string_lossy().into_owned(),
    ]);
    a
}

/// Convert a subtitle to WebVTT, caching at `cache_dir/{key}.vtt`. Returns the cached path.
///
/// `key` must uniquely identify the source content (e.g. `stream-{id}-{mtime}`).
pub async fn to_vtt(
    ffmpeg: &str,
    src: &SubtitleSource,
    cache_dir: &Path,
    key: &str,
) -> Result<PathBuf, PlaybackError> {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let out = cache_dir.join(format!("{safe}.vtt"));
    if tokio::fs::metadata(&out)
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        return Ok(out);
    }
    tokio::fs::create_dir_all(cache_dir).await?;
    let tmp = cache_dir.join(format!("{safe}.{}.tmp.vtt", uuid::Uuid::new_v4().simple()));
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new(ffmpeg)
            .args(extract_args(src, &tmp))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| PlaybackError::Ffmpeg("subtitle conversion timed out".into()))??;
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(PlaybackError::Ffmpeg(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    tokio::fs::rename(&tmp, &out).await?;
    Ok(out)
}

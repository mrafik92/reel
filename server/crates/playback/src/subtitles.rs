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

/// ffmpeg arguments for the conversion. `src` paths must already be local files or loopback
/// upstream-proxy URLs (see [`crate::media::MediaCtx::proxied_input`]).
pub fn extract_args(src: &SubtitleSource, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "error",
        "-y",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let path = match src {
        SubtitleSource::Embedded { path, .. } | SubtitleSource::External { path } => path,
    };
    a.extend([
        "-protocol_whitelist".to_string(),
        crate::ffmpeg::protocol_whitelist(crate::ffmpeg::is_remote(path)).to_string(),
    ]);
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

fn safe_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn cached(out: &Path) -> bool {
    tokio::fs::metadata(out)
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

async fn run_convert(ffmpeg: &str, src: &SubtitleSource, tmp: &Path) -> Result<(), PlaybackError> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new(ffmpeg)
            .args(extract_args(src, tmp))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| PlaybackError::ffmpeg("subtitle conversion timed out"))??;
    if !output.status.success() {
        let _ = tokio::fs::remove_file(tmp).await;
        return Err(PlaybackError::ffmpeg(format!(
            "subtitle conversion failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Convert a **local** subtitle to WebVTT, caching at `cache_dir/{key}.vtt`.
/// Remote (provider) sources are refused here: use [`to_vtt_with`], which goes through the
/// upstream proxy and the shared process budget.
pub async fn to_vtt(
    ffmpeg: &str,
    src: &SubtitleSource,
    cache_dir: &Path,
    key: &str,
) -> Result<PathBuf, PlaybackError> {
    let path = match src {
        SubtitleSource::Embedded { path, .. } | SubtitleSource::External { path } => path,
    };
    if crate::ffmpeg::is_remote(path) {
        return Err(PlaybackError::Invalid(
            "remote subtitle sources need the upstream proxy".into(),
        ));
    }
    let out = cache_dir.join(format!("{}.vtt", safe_key(key)));
    if cached(&out).await {
        return Ok(out);
    }
    tokio::fs::create_dir_all(cache_dir).await?;
    let tmp = cache_dir.join(format!(
        "{}.{}.tmp.vtt",
        safe_key(key),
        uuid::Uuid::new_v4().simple()
    ));
    run_convert(ffmpeg, src, &tmp).await?;
    tokio::fs::rename(&tmp, &out).await?;
    Ok(out)
}

/// Convert a subtitle to WebVTT (cached at `cache_dir/{key}.vtt`) with all the guards:
/// - single-flight per cache key (concurrent requests share one conversion),
/// - a slot of the global ffmpeg budget, waiting in a bounded queue (`Busy` if it stays full),
/// - remote sources are fetched through the loopback upstream proxy (no provider URL in argv).
pub async fn to_vtt_with(
    media: &crate::media::MediaCtx,
    ffmpeg: &str,
    src: &SubtitleSource,
    cache_dir: &Path,
    key: &str,
) -> Result<PathBuf, PlaybackError> {
    to_vtt_with_ua(media, ffmpeg, src, cache_dir, key, None).await
}

/// [`to_vtt_with`] with the User-Agent the proxy sends for a remote source (`None`:
/// [`crate::ffmpeg::REMOTE_UA`]), e.g. an IPTV source's configured one.
pub async fn to_vtt_with_ua(
    media: &crate::media::MediaCtx,
    ffmpeg: &str,
    src: &SubtitleSource,
    cache_dir: &Path,
    key: &str,
    user_agent: Option<&str>,
) -> Result<PathBuf, PlaybackError> {
    let ua = user_agent
        .filter(|ua| !ua.trim().is_empty())
        .unwrap_or(crate::ffmpeg::REMOTE_UA);
    let safe = safe_key(key);
    let out = cache_dir.join(format!("{safe}.vtt"));
    if cached(&out).await {
        return Ok(out);
    }
    let flight = media.flight(&safe);
    let _one = flight.lock().await;
    if cached(&out).await {
        return Ok(out); // another request finished the conversion while we waited
    }
    let _permit = media.proc(media.queue_wait()).await?;
    tokio::fs::create_dir_all(cache_dir).await?;
    let (resolved, _lease) = match src {
        SubtitleSource::Embedded { path, stream_index } => {
            let (p, l) = media.proxied_input(path, ua, Duration::from_secs(300))?;
            (
                SubtitleSource::Embedded {
                    path: p,
                    stream_index: *stream_index,
                },
                l,
            )
        }
        SubtitleSource::External { path } => {
            let (p, l) = media.proxied_input(path, ua, Duration::from_secs(300))?;
            (SubtitleSource::External { path: p }, l)
        }
    };
    let tmp = cache_dir.join(format!("{safe}.{}.tmp.vtt", uuid::Uuid::new_v4().simple()));
    run_convert(ffmpeg, &resolved, &tmp).await?;
    tokio::fs::rename(&tmp, &out).await?;
    Ok(out)
}

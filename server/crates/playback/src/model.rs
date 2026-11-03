//! Plain data types mirroring the `media_files` / `streams` rows the playback engine needs.

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Hdr {
    #[default]
    None,
    Hdr10,
    Hlg,
    Dv,
}

impl Hdr {
    pub fn parse(s: &str) -> Hdr {
        match s {
            "hdr10" => Hdr::Hdr10,
            "hlg" => Hdr::Hlg,
            "dv" => Hdr::Dv,
            _ => Hdr::None,
        }
    }
}

/// One row of the `streams` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StreamInfo {
    pub id: i64,
    /// ffprobe stream index inside the container (`streams.idx`).
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
    pub external_path: Option<String>,
}

impl StreamInfo {
    pub fn new(idx: i64, kind: StreamKind, codec: &str) -> Self {
        StreamInfo {
            id: idx,
            idx,
            kind,
            codec: Some(codec.to_string()),
            profile: None,
            level: None,
            width: None,
            height: None,
            hdr: Hdr::None,
            bit_depth: None,
            channels: None,
            language: None,
            title: None,
            default: false,
            forced: false,
            external_path: None,
        }
    }
}

/// One row of `media_files` (bitrate in bits/second as stored by the scanner).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FileInfo {
    pub id: i64,
    pub path: String,
    pub container: Option<String>,
    pub duration_s: Option<f64>,
    pub bitrate: Option<i64>,
}

/// Load a file and its streams from the database.
pub async fn load_file(
    pool: &SqlitePool,
    file_id: i64,
) -> Result<Option<(FileInfo, Vec<StreamInfo>)>, sqlx::Error> {
    let Some(row) = sqlx::query(
        "SELECT id, path, container, duration_s, bitrate, size FROM media_files WHERE id = ?",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let file = FileInfo {
        id: row.get("id"),
        path: row.get("path"),
        container: row.get("container"),
        duration_s: row.get("duration_s"),
        bitrate: bitrate_or_estimate(
            row.get("bitrate"),
            row.get::<Option<i64>, _>("size"),
            row.get("duration_s"),
        ),
    };
    let rows = sqlx::query(
        "SELECT id, idx, kind, codec, profile, level, width, height, hdr, bit_depth, channels, \
         language, title, \"default\" AS is_default, forced, external_path \
         FROM streams WHERE file_id = ? ORDER BY idx",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    let streams = rows
        .into_iter()
        .map(|r| {
            let kind: String = r.get("kind");
            let hdr: String = r.get("hdr");
            StreamInfo {
                id: r.get("id"),
                idx: r.get("idx"),
                kind: match kind.as_str() {
                    "video" => StreamKind::Video,
                    "audio" => StreamKind::Audio,
                    _ => StreamKind::Subtitle,
                },
                codec: r.get("codec"),
                profile: r.get("profile"),
                level: r.get("level"),
                width: r.get("width"),
                height: r.get("height"),
                hdr: Hdr::parse(&hdr),
                bit_depth: r.get("bit_depth"),
                channels: r.get("channels"),
                language: r.get("language"),
                title: r.get("title"),
                default: r.get::<i64, _>("is_default") != 0,
                forced: r.get::<i64, _>("forced") != 0,
                external_path: r.get("external_path"),
            }
        })
        .collect();
    Ok(Some((file, streams)))
}

/// Probe bitrate (bits/s), or `size * 8 / duration` when the probe did not report one.
pub fn bitrate_or_estimate(
    bitrate: Option<i64>,
    size: Option<i64>,
    duration_s: Option<f64>,
) -> Option<i64> {
    match (bitrate.filter(|b| *b > 0), size, duration_s) {
        (Some(b), _, _) => Some(b),
        (None, Some(sz), Some(d)) if sz > 0 && d > 1.0 => Some((sz as f64 * 8.0 / d) as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_falls_back_to_size_over_duration() {
        assert_eq!(
            bitrate_or_estimate(Some(9_000_000), Some(1), Some(1.0)),
            Some(9_000_000)
        );
        // 6.7 GB over 5629 s ≈ 9.5 Mbps
        let b = bitrate_or_estimate(None, Some(6_700_000_000), Some(5629.0)).unwrap();
        assert!((9_400_000..9_600_000).contains(&b), "{b}");
        assert_eq!(bitrate_or_estimate(Some(0), Some(100), Some(0.0)), None);
        assert_eq!(bitrate_or_estimate(None, None, Some(10.0)), None);
    }
}

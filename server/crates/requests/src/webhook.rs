//! Radarr / Sonarr webhook payload parsing.

use serde_json::Value;

use crate::{ArrKind, Error, Result};

/// Webhook `eventType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrEventKind {
    Grab,
    Download,
    DownloadFailed,
    FileDelete,
    Test,
    Other(String),
}

/// Parsed webhook event.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrEvent {
    pub kind: ArrEventKind,
    /// Inferred from the payload (`movie` → Radarr, `series` → Sonarr).
    pub source: Option<ArrKind>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    /// Radarr movieId / Sonarr seriesId.
    pub arr_item_id: Option<i64>,
    pub title: Option<String>,
    pub download_id: Option<String>,
    pub release_title: Option<String>,
    pub size: Option<u64>,
    pub message: Option<String>,
    /// Files imported (Download) or removed (FileDelete).
    pub paths: Vec<String>,
    pub seasons: Vec<i64>,
    pub is_upgrade: bool,
    pub delete_reason: Option<String>,
}

fn s(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

fn n(v: &Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(Value::as_i64).filter(|x| *x > 0)
}

fn join(folder: Option<String>, rel: Option<String>) -> Option<String> {
    match (folder, rel) {
        (Some(f), Some(r)) => Some(format!("{}/{}", f.trim_end_matches(['/', '\\']), r)),
        _ => None,
    }
}

/// Parse a webhook body.
pub fn parse(body: &[u8]) -> Result<ArrEvent> {
    let v: Value = serde_json::from_slice(body).map_err(|e| Error::Parse(e.to_string()))?;
    let et = v
        .get("eventType")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse("missing eventType".into()))?;
    let kind = match et {
        "Grab" => ArrEventKind::Grab,
        "Download" => ArrEventKind::Download,
        "DownloadFailed" => ArrEventKind::DownloadFailed,
        "MovieFileDelete" | "EpisodeFileDelete" => ArrEventKind::FileDelete,
        "Test" => ArrEventKind::Test,
        other => ArrEventKind::Other(other.to_string()),
    };
    let source = if v.get("movie").is_some() {
        Some(ArrKind::Radarr)
    } else if v.get("series").is_some() {
        Some(ArrKind::Sonarr)
    } else {
        None
    };

    // Collect file paths from every supported shape (Radarr movieFile, Sonarr episodeFile(s)).
    let series_path = s(&v, "/series/path");
    let mut collected: Vec<Option<String>> = vec![
        s(&v, "/movieFile/path")
            .or_else(|| join(s(&v, "/movie/folderPath"), s(&v, "/movieFile/relativePath"))),
        s(&v, "/episodeFile/path")
            .or_else(|| join(series_path.clone(), s(&v, "/episodeFile/relativePath"))),
    ];
    for f in v
        .get("episodeFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collected.push(s(f, "/path").or_else(|| join(series_path.clone(), s(f, "/relativePath"))));
    }
    let mut paths: Vec<String> = Vec::new();
    for p in collected.into_iter().flatten() {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }

    let mut seasons: Vec<i64> = v
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("seasonNumber").and_then(Value::as_i64))
        .collect();
    seasons.sort_unstable();
    seasons.dedup();

    let (tmdb_id, tvdb_id, arr_item_id, title) = match source {
        Some(ArrKind::Radarr) => (
            n(&v, "/movie/tmdbId").or_else(|| n(&v, "/remoteMovie/tmdbId")),
            None,
            n(&v, "/movie/id"),
            s(&v, "/movie/title"),
        ),
        Some(ArrKind::Sonarr) => (
            n(&v, "/series/tmdbId"),
            n(&v, "/series/tvdbId"),
            n(&v, "/series/id"),
            s(&v, "/series/title"),
        ),
        None => (None, None, None, None),
    };

    Ok(ArrEvent {
        kind,
        source,
        tmdb_id,
        tvdb_id,
        arr_item_id,
        title,
        download_id: s(&v, "/downloadId"),
        release_title: s(&v, "/release/releaseTitle"),
        size: v.pointer("/release/size").and_then(Value::as_u64),
        message: s(&v, "/message"),
        paths,
        seasons,
        is_upgrade: v.get("isUpgrade").and_then(Value::as_bool).unwrap_or(false),
        delete_reason: s(&v, "/deleteReason"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(v: Value) -> ArrEvent {
        parse(v.to_string().as_bytes()).unwrap()
    }

    #[test]
    fn radarr_grab() {
        let e = p(json!({
            "eventType":"Grab","movie":{"id":7,"title":"Dune","tmdbId":438631},
            "release":{"releaseTitle":"Dune.2021.1080p","size":8000000000u64},
            "downloadId":"ABCDEF"
        }));
        assert_eq!(e.kind, ArrEventKind::Grab);
        assert_eq!(e.source, Some(ArrKind::Radarr));
        assert_eq!((e.tmdb_id, e.arr_item_id), (Some(438631), Some(7)));
        assert_eq!(e.release_title.as_deref(), Some("Dune.2021.1080p"));
        assert_eq!(e.download_id.as_deref(), Some("ABCDEF"));
        assert_eq!(e.size, Some(8_000_000_000));
    }

    #[test]
    fn radarr_download_paths() {
        let e = p(json!({
            "eventType":"Download","isUpgrade":true,
            "movie":{"id":7,"title":"Dune","tmdbId":438631,"folderPath":"/movies/Dune (2021)"},
            "movieFile":{"relativePath":"Dune.mkv"}
        }));
        assert_eq!(e.kind, ArrEventKind::Download);
        assert_eq!(e.paths, vec!["/movies/Dune (2021)/Dune.mkv"]);
        assert!(e.is_upgrade);
        let e = p(json!({
            "eventType":"Download","movie":{"id":7,"tmdbId":1},
            "movieFile":{"path":"/m/a.mkv","relativePath":"a.mkv"}
        }));
        assert_eq!(e.paths, vec!["/m/a.mkv"]);
    }

    #[test]
    fn radarr_failed_and_delete() {
        let e =
            p(json!({"eventType":"DownloadFailed","movie":{"id":1,"tmdbId":2},"message":"bad"}));
        assert_eq!(e.kind, ArrEventKind::DownloadFailed);
        assert_eq!(e.message.as_deref(), Some("bad"));
        let e = p(
            json!({"eventType":"MovieFileDelete","movie":{"id":1,"tmdbId":2},
            "movieFile":{"path":"/m/a.mkv"},"deleteReason":"manual"}),
        );
        assert_eq!(e.kind, ArrEventKind::FileDelete);
        assert_eq!(e.paths, vec!["/m/a.mkv"]);
        assert_eq!(e.delete_reason.as_deref(), Some("manual"));
    }

    #[test]
    fn sonarr_download_multi() {
        let e = p(json!({
            "eventType":"Download",
            "series":{"id":3,"title":"Show","tvdbId":81189,"tmdbId":1396,"path":"/tv/Show"},
            "episodes":[{"seasonNumber":1,"episodeNumber":1},{"seasonNumber":1,"episodeNumber":2},{"seasonNumber":2,"episodeNumber":1}],
            "episodeFiles":[{"relativePath":"S01E01.mkv"},{"path":"/tv/Show/S01E02.mkv"}]
        }));
        assert_eq!(e.source, Some(ArrKind::Sonarr));
        assert_eq!(
            (e.tvdb_id, e.tmdb_id, e.arr_item_id),
            (Some(81189), Some(1396), Some(3))
        );
        assert_eq!(e.seasons, vec![1, 2]);
        assert_eq!(e.paths, vec!["/tv/Show/S01E01.mkv", "/tv/Show/S01E02.mkv"]);
        let e = p(
            json!({"eventType":"EpisodeFileDelete","series":{"id":3,"tvdbId":1,"path":"/tv/S"},
            "episodeFile":{"relativePath":"a.mkv"}}),
        );
        assert_eq!(e.kind, ArrEventKind::FileDelete);
        assert_eq!(e.paths, vec!["/tv/S/a.mkv"]);
    }

    #[test]
    fn test_and_garbage() {
        assert_eq!(p(json!({"eventType":"Test"})).kind, ArrEventKind::Test);
        assert_eq!(
            p(json!({"eventType":"Rename"})).kind,
            ArrEventKind::Other("Rename".into())
        );
        assert!(parse(b"nope").is_err());
        assert!(parse(b"{}").is_err());
    }
}

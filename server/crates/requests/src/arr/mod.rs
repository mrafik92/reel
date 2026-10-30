//! Radarr / Sonarr v3 API clients.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{Error, Result};

pub mod radarr;
pub mod sonarr;

pub use radarr::{AddMovieOptions, Radarr, RadarrMovie};
pub use sonarr::{AddSeriesOptions, Sonarr, SonarrEpisode};

/// Thin authenticated HTTP client shared by Radarr and Sonarr.
#[derive(Debug, Clone)]
pub struct ArrClient {
    http: reqwest::Client,
    base: String,
    key: String,
    page_size: u32,
}

impl ArrClient {
    pub fn new(base: impl Into<String>, key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("reel/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            key: key.into(),
            page_size: 500,
        }
    }

    pub fn with_page_size(mut self, n: u32) -> Self {
        self.page_size = n.max(1);
        self
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let resp = req.header("X-Api-Key", &self.key).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let mut body = resp.text().await.unwrap_or_default();
            body.truncate(500);
            return Err(Error::Remote {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json().await?)
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.send(self.http.get(format!("{}{}", self.base, path)).query(query))
            .await
    }

    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.send(self.http.post(format!("{}{}", self.base, path)).json(body))
            .await
    }

    pub async fn put<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.send(self.http.put(format!("{}{}", self.base, path)).json(body))
            .await
    }

    pub async fn quality_profiles(&self) -> Result<Vec<QualityProfile>> {
        self.get("/api/v3/qualityprofile", &[]).await
    }

    pub async fn root_folders(&self) -> Result<Vec<RootFolder>> {
        self.get("/api/v3/rootfolder", &[]).await
    }

    /// Whole queue (all pages). `extra` carries `includeMovie` / `includeEpisode` flags.
    pub async fn queue(&self, extra: &[(&str, &str)]) -> Result<Vec<QueueItem>> {
        let mut out = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q: Vec<(&str, String)> = vec![
                ("page", page.to_string()),
                ("pageSize", self.page_size.to_string()),
            ];
            q.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
            let p: QueuePage = self.get("/api/v3/queue", &q).await?;
            let n = p.records.len();
            out.extend(p.records);
            if n == 0 || out.len() as u64 >= p.total_records || n < self.page_size as usize {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    /// Status of one command.
    pub async fn command(&self, id: i64) -> Result<Command> {
        self.get(&format!("/api/v3/command/{id}"), &[]).await
    }

    /// Recent commands.
    pub async fn commands(&self) -> Result<Vec<Command>> {
        self.get("/api/v3/command", &[]).await
    }

    /// Run a named command with extra body fields, e.g. `MoviesSearch`.
    pub async fn run_command(&self, name: &str, fields: Value) -> Result<Command> {
        let mut body = json!({ "name": name });
        if let (Some(b), Some(f)) = (body.as_object_mut(), fields.as_object()) {
            for (k, v) in f {
                b.insert(k.clone(), v.clone());
            }
        }
        self.post("/api/v3/command", &body).await
    }
}

/// Quality profile (id + name only).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct QualityProfile {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

/// Root folder.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RootFolder {
    #[serde(default)]
    pub id: i64,
    pub path: String,
    #[serde(default)]
    pub free_space: Option<i64>,
}

/// Command resource (subset).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Command {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub body: Value,
}

impl Command {
    /// Finished (successfully or not).
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status.to_ascii_lowercase().as_str(),
            "completed" | "failed" | "aborted" | "cancelled" | "orphaned"
        )
    }

    /// Does this search command target the given Arr item?
    pub fn targets(&self, item_id: i64) -> bool {
        let is_search = self.name.to_ascii_lowercase().contains("search");
        if !is_search {
            return false;
        }
        let b = &self.body;
        b.get("movieIds")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_i64() == Some(item_id)))
            || b.get("seriesId").and_then(Value::as_i64) == Some(item_id)
    }
}

/// Paged queue response.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct QueuePage {
    total_records: u64,
    records: Vec<QueueItem>,
}

/// Queue record (fields Reel uses).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueItem {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub season_number: Option<i64>,
    pub title: Option<String>,
    pub size: f64,
    pub sizeleft: f64,
    pub timeleft: Option<String>,
    pub status: Option<String>,
    pub tracked_download_state: Option<String>,
    pub tracked_download_status: Option<String>,
    pub download_id: Option<String>,
    pub error_message: Option<String>,
    pub episode: Option<QueueEpisode>,
    /// Radarr (`includeMovie=true`).
    pub movie: Option<QueueMedia>,
    /// Sonarr (`includeSeries=true`).
    pub series: Option<QueueMedia>,
}

/// Movie / series embedded in a queue record.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueMedia {
    pub title: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub images: Vec<QueueImage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueImage {
    pub cover_type: Option<String>,
    pub remote_url: Option<String>,
}

/// Episode embedded with `includeEpisode=true`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueEpisode {
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    pub title: Option<String>,
}

impl QueueItem {
    /// Season this item belongs to.
    pub fn season(&self) -> Option<i64> {
        self.season_number
            .or_else(|| self.episode.as_ref().and_then(|e| e.season_number))
    }
    pub fn episode_number(&self) -> Option<i64> {
        self.episode.as_ref().and_then(|e| e.episode_number)
    }
}

/// Parse Arr `timeleft` (`[d.]hh:mm:ss`) into seconds.
pub fn parse_timeleft(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, rest) = match s.split_once('.') {
        Some((d, r)) if r.contains(':') => (d.parse::<u64>().ok()?, r),
        _ => (0, s),
    };
    let mut parts = rest.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(days * 86_400 + h * 3600 + m * 60 + sec as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeleft() {
        assert_eq!(parse_timeleft("00:12:34"), Some(754));
        assert_eq!(parse_timeleft("1.02:03:04"), Some(86400 + 7384));
        assert_eq!(parse_timeleft("00:00:05.123"), Some(5));
        assert_eq!(parse_timeleft("junk"), None);
    }
}

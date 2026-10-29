//! Sonarr v3 client.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ArrClient, Command, QualityProfile, QueueItem, RootFolder};
use crate::Result;

/// Episode resource (subset).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SonarrEpisode {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    pub title: Option<String>,
    pub monitored: bool,
    pub has_file: bool,
}

/// Options for adding a series.
#[derive(Debug, Clone)]
pub struct AddSeriesOptions {
    pub quality_profile_id: i64,
    pub root_folder_path: String,
    pub monitored: bool,
    /// Seasons to monitor; `None` = every non-special season.
    pub seasons: Option<Vec<i64>>,
    pub search: bool,
}

impl AddSeriesOptions {
    pub fn new(quality_profile_id: i64, root_folder_path: impl Into<String>) -> Self {
        Self {
            quality_profile_id,
            root_folder_path: root_folder_path.into(),
            monitored: true,
            seasons: None,
            search: true,
        }
    }
}

/// Set `monitored` on each entry of `seasons` according to the selection.
pub fn apply_season_selection(seasons: &mut Value, selection: Option<&[i64]>, keep_existing: bool) {
    if let Some(arr) = seasons.as_array_mut() {
        for s in arr {
            let n = s.get("seasonNumber").and_then(Value::as_i64).unwrap_or(-1);
            let wanted = match selection {
                Some(sel) => sel.contains(&n),
                None => n > 0,
            };
            let was = s.get("monitored").and_then(Value::as_bool).unwrap_or(false);
            if let Some(o) = s.as_object_mut() {
                o.insert("monitored".into(), json!(wanted || (keep_existing && was)));
            }
        }
    }
}

/// Sonarr API.
#[derive(Debug, Clone)]
pub struct Sonarr(pub ArrClient);

impl Sonarr {
    pub fn new(url: impl Into<String>, key: impl Into<String>) -> Self {
        Sonarr(ArrClient::new(url, key))
    }

    /// Raw lookup resource for a TVDB id.
    pub async fn lookup_tvdb(&self, tvdb_id: i64) -> Result<Option<Value>> {
        let v: Vec<Value> = self
            .0
            .get(
                "/api/v3/series/lookup",
                &[("term", format!("tvdb:{tvdb_id}"))],
            )
            .await?;
        Ok(v.into_iter().next())
    }

    /// Series already in Sonarr, by TVDB id (raw resource, so it can be PUT back).
    pub async fn series_by_tvdb(&self, tvdb_id: i64) -> Result<Option<Value>> {
        let v: Vec<Value> = self
            .0
            .get("/api/v3/series", &[("tvdbId", tvdb_id.to_string())])
            .await?;
        Ok(v.into_iter()
            .find(|s| s.get("tvdbId").and_then(Value::as_i64) == Some(tvdb_id)))
    }

    pub async fn series(&self, id: i64) -> Result<Value> {
        self.0.get(&format!("/api/v3/series/{id}"), &[]).await
    }

    pub async fn update_series(&self, id: i64, series: &Value) -> Result<Value> {
        self.0.put(&format!("/api/v3/series/{id}"), series).await
    }

    /// POST /series using a lookup resource as the base body.
    pub async fn add_series(&self, lookup: Value, opts: &AddSeriesOptions) -> Result<Value> {
        let mut body = lookup;
        if let Some(o) = body.as_object_mut() {
            o.insert("qualityProfileId".into(), json!(opts.quality_profile_id));
            o.insert("rootFolderPath".into(), json!(opts.root_folder_path));
            o.insert("monitored".into(), json!(opts.monitored));
            o.entry("seasonFolder").or_insert(json!(true));
            o.insert(
                "addOptions".into(),
                json!({
                    "searchForMissingEpisodes": opts.search,
                    "ignoreEpisodesWithFiles": true,
                }),
            );
            if let Some(seasons) = o.get_mut("seasons") {
                apply_season_selection(seasons, opts.seasons.as_deref(), false);
            }
        }
        self.0.post("/api/v3/series", &body).await
    }

    pub async fn episodes(&self, series_id: i64) -> Result<Vec<SonarrEpisode>> {
        self.0
            .get("/api/v3/episode", &[("seriesId", series_id.to_string())])
            .await
    }

    pub async fn quality_profiles(&self) -> Result<Vec<QualityProfile>> {
        self.0.quality_profiles().await
    }

    pub async fn root_folders(&self) -> Result<Vec<RootFolder>> {
        self.0.root_folders().await
    }

    pub async fn queue(&self) -> Result<Vec<QueueItem>> {
        self.0
            .queue(&[("includeEpisode", "true"), ("includeSeries", "true")])
            .await
    }

    pub async fn search_series(&self, series_id: i64) -> Result<Command> {
        self.0
            .run_command("SeriesSearch", json!({ "seriesId": series_id }))
            .await
    }

    pub async fn search_season(&self, series_id: i64, season: i64) -> Result<Command> {
        self.0
            .run_command(
                "SeasonSearch",
                json!({ "seriesId": series_id, "seasonNumber": season }),
            )
            .await
    }
}

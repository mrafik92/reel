//! Radarr v3 client.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ArrClient, Command, QualityProfile, QueueItem, RootFolder};
use crate::Result;

/// Movie resource (subset).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RadarrMovie {
    pub id: i64,
    pub title: String,
    pub tmdb_id: i64,
    pub year: Option<i64>,
    pub monitored: bool,
    pub has_file: bool,
    pub path: Option<String>,
}

/// Options for adding a movie.
#[derive(Debug, Clone)]
pub struct AddMovieOptions {
    pub quality_profile_id: i64,
    pub root_folder_path: String,
    pub monitored: bool,
    /// `announced` | `inCinemas` | `released`
    pub minimum_availability: String,
    pub search: bool,
}

impl AddMovieOptions {
    pub fn new(quality_profile_id: i64, root_folder_path: impl Into<String>) -> Self {
        Self {
            quality_profile_id,
            root_folder_path: root_folder_path.into(),
            monitored: true,
            minimum_availability: "released".into(),
            search: true,
        }
    }
}

/// Radarr API.
#[derive(Debug, Clone)]
pub struct Radarr(pub ArrClient);

impl Radarr {
    pub fn new(url: impl Into<String>, key: impl Into<String>) -> Self {
        Radarr(ArrClient::new(url, key))
    }

    /// Raw lookup resource for a TMDB id (object or first array element).
    pub async fn lookup_tmdb(&self, tmdb_id: i64) -> Result<Option<Value>> {
        let v: Value = self
            .0
            .get(
                "/api/v3/movie/lookup/tmdb",
                &[("tmdbId", tmdb_id.to_string())],
            )
            .await?;
        Ok(match v {
            Value::Array(mut a) if !a.is_empty() => Some(a.remove(0)),
            Value::Object(_) => Some(v),
            _ => None,
        })
    }

    pub async fn movies(&self) -> Result<Vec<RadarrMovie>> {
        self.0.get("/api/v3/movie", &[]).await
    }

    pub async fn movie(&self, id: i64) -> Result<RadarrMovie> {
        self.0.get(&format!("/api/v3/movie/{id}"), &[]).await
    }

    /// Movie already in Radarr, by TMDB id.
    pub async fn movie_by_tmdb(&self, tmdb_id: i64) -> Result<Option<RadarrMovie>> {
        let v: Vec<RadarrMovie> = self
            .0
            .get("/api/v3/movie", &[("tmdbId", tmdb_id.to_string())])
            .await?;
        Ok(v.into_iter().find(|m| m.tmdb_id == tmdb_id))
    }

    /// POST /movie using a lookup resource as the base body.
    pub async fn add_movie(&self, lookup: Value, opts: &AddMovieOptions) -> Result<RadarrMovie> {
        let mut body = lookup;
        if let Some(o) = body.as_object_mut() {
            o.insert("qualityProfileId".into(), json!(opts.quality_profile_id));
            o.insert("rootFolderPath".into(), json!(opts.root_folder_path));
            o.insert("monitored".into(), json!(opts.monitored));
            o.insert(
                "minimumAvailability".into(),
                json!(opts.minimum_availability),
            );
            o.insert(
                "addOptions".into(),
                json!({ "searchForMovie": opts.search }),
            );
        }
        self.0.post("/api/v3/movie", &body).await
    }

    pub async fn quality_profiles(&self) -> Result<Vec<QualityProfile>> {
        self.0.quality_profiles().await
    }

    pub async fn root_folders(&self) -> Result<Vec<RootFolder>> {
        self.0.root_folders().await
    }

    pub async fn queue(&self) -> Result<Vec<QueueItem>> {
        self.0.queue(&[("includeMovie", "true")]).await
    }

    /// Trigger `MoviesSearch`.
    pub async fn search_movie(&self, movie_id: i64) -> Result<Command> {
        self.0
            .run_command("MoviesSearch", json!({ "movieIds": [movie_id] }))
            .await
    }
}

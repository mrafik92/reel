//! Pushing approved requests to Radarr / Sonarr.

use serde_json::Value;

use crate::{
    ArrKind, Error, MediaType, Result,
    arr::{AddMovieOptions, AddSeriesOptions, Radarr, Sonarr, sonarr::apply_season_selection},
    models::{ARR_COLS, ArrServer, ArrServerRow, RequestRecord},
    shared::Shared,
};

/// Outcome of pushing a request to an Arr server.
#[derive(Debug, Clone)]
pub(crate) struct Dispatched {
    pub server: ArrServer,
    pub item_id: i64,
    /// Already downloaded in the Arr (no search needed).
    pub has_file: bool,
    pub profile: Option<i64>,
    pub root: Option<String>,
}

impl Shared {
    /// Choose the Arr server: explicit `arr_id`, else the first non-4K of the right kind, else any.
    pub async fn pick_server(&self, kind: ArrKind, preferred: Option<i64>) -> Result<ArrServer> {
        if let Some(id) = preferred {
            if let Some(s) = self.get_server(id).await? {
                return Ok(s);
            }
        }
        let row: Option<ArrServerRow> = sqlx::query_as(&format!(
            "SELECT {ARR_COLS} FROM arr_servers WHERE kind = ? ORDER BY is_4k, id LIMIT 1"
        ))
        .bind(kind.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(Into::into)
            .ok_or_else(|| Error::NotConfigured(format!("no {} server configured", kind.as_str())))
    }

    /// Resolve the TVDB id of a TMDB series.
    async fn tvdb_for(&self, tmdb_id: i64) -> Result<i64> {
        let tmdb = self
            .tmdb
            .as_ref()
            .ok_or_else(|| Error::NotConfigured("TMDB is not configured".into()))?;
        let tv = tmdb.tv(tmdb_id).await?;
        tv.external_ids
            .and_then(|e| e.tvdb_id)
            .ok_or_else(|| Error::NotFound(format!("TVDB id for TMDB tv {tmdb_id}")))
    }

    /// Add the title to its Arr (or attach to the existing entry) and start a search.
    pub async fn dispatch(&self, req: &RequestRecord) -> Result<Dispatched> {
        let server = self
            .pick_server(req.media_type.arr_kind(), req.arr_id)
            .await?;
        match req.media_type {
            MediaType::Movie => self.dispatch_movie(req, server).await,
            MediaType::Tv => self.dispatch_tv(req, server).await,
        }
    }

    async fn dispatch_movie(&self, req: &RequestRecord, server: ArrServer) -> Result<Dispatched> {
        let r = Radarr::new(&server.url, &server.api_key);
        if let Some(m) = r.movie_by_tmdb(req.tmdb_id).await? {
            if !m.has_file {
                r.search_movie(m.id).await?;
            }
            return Ok(Dispatched {
                server,
                item_id: m.id,
                has_file: m.has_file,
                profile: None,
                root: None,
            });
        }
        let lookup = r
            .lookup_tmdb(req.tmdb_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("radarr lookup for tmdb {}", req.tmdb_id)))?;
        let profile = match req.quality_profile_id.or(server.default_profile) {
            Some(p) => p,
            None => r
                .quality_profiles()
                .await?
                .first()
                .map(|p| p.id)
                .ok_or_else(|| Error::NotConfigured("radarr has no quality profiles".into()))?,
        };
        let root = match req.root_folder.clone().or(server.default_root.clone()) {
            Some(p) => p,
            None => r
                .root_folders()
                .await?
                .first()
                .map(|f| f.path.clone())
                .ok_or_else(|| Error::NotConfigured("radarr has no root folders".into()))?,
        };
        let movie = r
            .add_movie(lookup, &AddMovieOptions::new(profile, root.clone()))
            .await?;
        Ok(Dispatched {
            server,
            item_id: movie.id,
            has_file: false,
            profile: Some(profile),
            root: Some(root),
        })
    }

    async fn dispatch_tv(&self, req: &RequestRecord, server: ArrServer) -> Result<Dispatched> {
        let s = Sonarr::new(&server.url, &server.api_key);
        let tvdb = self.tvdb_for(req.tmdb_id).await?;
        if let Some(mut series) = s.series_by_tvdb(tvdb).await? {
            let id = series
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::Parse("sonarr series without id".into()))?;
            if let Some(o) = series.as_object_mut() {
                o.insert("monitored".into(), Value::Bool(true));
                if let Some(seasons) = o.get_mut("seasons") {
                    apply_season_selection(seasons, req.seasons.as_deref(), true);
                }
            }
            s.update_series(id, &series).await?;
            self.search_series_scope(&s, id, req.seasons.as_deref())
                .await?;
            return Ok(Dispatched {
                server,
                item_id: id,
                has_file: false,
                profile: None,
                root: None,
            });
        }
        let lookup = s
            .lookup_tvdb(tvdb)
            .await?
            .ok_or_else(|| Error::NotFound(format!("sonarr lookup for tvdb {tvdb}")))?;
        let profile = match req.quality_profile_id.or(server.default_profile) {
            Some(p) => p,
            None => s
                .quality_profiles()
                .await?
                .first()
                .map(|p| p.id)
                .ok_or_else(|| Error::NotConfigured("sonarr has no quality profiles".into()))?,
        };
        let root = match req.root_folder.clone().or(server.default_root.clone()) {
            Some(p) => p,
            None => s
                .root_folders()
                .await?
                .first()
                .map(|f| f.path.clone())
                .ok_or_else(|| Error::NotConfigured("sonarr has no root folders".into()))?,
        };
        let mut opts = AddSeriesOptions::new(profile, root.clone());
        opts.seasons = req.seasons.clone();
        let created = s.add_series(lookup, &opts).await?;
        let id = created
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Parse("sonarr add returned no id".into()))?;
        Ok(Dispatched {
            server,
            item_id: id,
            has_file: false,
            profile: Some(profile),
            root: Some(root),
        })
    }

    async fn search_series_scope(
        &self,
        s: &Sonarr,
        series_id: i64,
        seasons: Option<&[i64]>,
    ) -> Result<()> {
        match seasons {
            Some(list) if !list.is_empty() => {
                for n in list {
                    s.search_season(series_id, *n).await?;
                }
            }
            _ => {
                s.search_series(series_id).await?;
            }
        }
        Ok(())
    }

    /// Trigger another Arr search for an already-pushed request.
    pub async fn search_again(
        &self,
        req: &RequestRecord,
        server: &ArrServer,
        item_id: i64,
    ) -> Result<()> {
        match req.media_type {
            MediaType::Movie => {
                Radarr::new(&server.url, &server.api_key)
                    .search_movie(item_id)
                    .await?;
            }
            MediaType::Tv => {
                let s = Sonarr::new(&server.url, &server.api_key);
                self.search_series_scope(&s, item_id, req.seasons.as_deref())
                    .await?;
            }
        }
        Ok(())
    }
}

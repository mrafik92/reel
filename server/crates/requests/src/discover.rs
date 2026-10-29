//! TMDB discovery with availability badges and live progress attached.

use std::collections::HashMap;

use reel_metadata::tmdb::{Paged, SearchResult};
use serde::Serialize;

use crate::{
    Error, MediaType, Result,
    models::Progress,
    service::{Availability, RequestsService},
    shared::SharedRef,
};

/// One discovery card.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoverCard {
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub status: Availability,
    /// Live request progress, if the title is being requested/downloaded.
    pub progress: Option<Progress>,
}

/// A page of cards.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoverPage {
    pub page: u32,
    pub total_pages: u32,
    pub total_results: u32,
    pub results: Vec<DiscoverCard>,
}

/// Detail for `GET /discover/{movie|tv}/{tmdbId}`.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoverDetail {
    pub card: DiscoverCard,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    /// TV: season numbers (excluding specials).
    pub seasons: Vec<i64>,
}

/// Discovery facade.
pub struct Discovery {
    s: SharedRef,
}

impl Discovery {
    pub(crate) fn new(s: SharedRef) -> Self {
        Discovery { s }
    }

    fn tmdb(&self) -> Result<&reel_metadata::TmdbClient> {
        self.s
            .tmdb
            .as_ref()
            .ok_or_else(|| Error::NotConfigured("TMDB is not configured".into()))
    }

    fn svc(&self) -> RequestsService {
        RequestsService { s: self.s.clone() }
    }

    async fn cards(
        &self,
        results: Vec<SearchResult>,
        default_type: Option<MediaType>,
    ) -> Result<Vec<DiscoverCard>> {
        let mut movies = vec![];
        let mut tv = vec![];
        let mut typed = vec![];
        for r in results {
            let mt = match r.media_type.as_deref() {
                Some("movie") => MediaType::Movie,
                Some("tv") => MediaType::Tv,
                Some(_) => continue, // people etc.
                None => match default_type {
                    Some(t) => t,
                    None => continue,
                },
            };
            match mt {
                MediaType::Movie => movies.push(r.id),
                MediaType::Tv => tv.push(r.id),
            }
            typed.push((mt, r));
        }
        let svc = self.svc();
        let mut badges: HashMap<(MediaType, i64), Availability> = HashMap::new();
        for (mt, ids) in [(MediaType::Movie, movies), (MediaType::Tv, tv)] {
            if ids.is_empty() {
                continue;
            }
            for (id, a) in svc.availability_for(mt, &ids).await? {
                badges.insert((mt, id), a);
            }
        }
        // Live progress by (media_type, tmdb_id).
        let live: HashMap<(MediaType, i64), Progress> = self
            .s
            .progress
            .iter()
            .map(|e| ((e.media_type, e.tmdb_id), e.value().clone()))
            .collect();
        Ok(typed
            .into_iter()
            .map(|(mt, r)| DiscoverCard {
                tmdb_id: r.id,
                media_type: mt,
                title: r.display_title().to_string(),
                year: r.year(),
                overview: r.overview.clone(),
                poster_path: r.poster_path.clone(),
                backdrop_path: r.backdrop_path.clone(),
                vote_average: r.vote_average,
                status: badges
                    .get(&(mt, r.id))
                    .copied()
                    .unwrap_or(Availability::None),
                progress: live.get(&(mt, r.id)).cloned(),
            })
            .collect())
    }

    async fn page(&self, p: Paged<SearchResult>, t: Option<MediaType>) -> Result<DiscoverPage> {
        Ok(DiscoverPage {
            page: p.page,
            total_pages: p.total_pages,
            total_results: p.total_results,
            results: self.cards(p.results, t).await?,
        })
    }

    /// `media` = `movie` | `tv` | `all`; `window` = `day` | `week`.
    pub async fn trending(&self, media: &str, window: &str, page: u32) -> Result<DiscoverPage> {
        let p = self.tmdb()?.trending(media, window, page).await?;
        let t = MediaType::parse(media);
        self.page(p, t).await
    }

    pub async fn popular(&self, media: MediaType, page: u32) -> Result<DiscoverPage> {
        let t = self.tmdb()?;
        let p = match media {
            MediaType::Movie => t.popular_movies(page).await?,
            MediaType::Tv => t.popular_tv(page).await?,
        };
        self.page(p, Some(media)).await
    }

    pub async fn upcoming(&self, page: u32) -> Result<DiscoverPage> {
        let p = self.tmdb()?.upcoming_movies(page).await?;
        self.page(p, Some(MediaType::Movie)).await
    }

    pub async fn by_genre(
        &self,
        media: MediaType,
        genre_id: i64,
        page: u32,
    ) -> Result<DiscoverPage> {
        let p = self
            .tmdb()?
            .discover(
                media == MediaType::Tv,
                &[
                    ("with_genres", genre_id.to_string()),
                    ("page", page.to_string()),
                    ("sort_by", "popularity.desc".into()),
                ],
            )
            .await?;
        self.page(p, Some(media)).await
    }

    /// Search movies and TV (or one type), most popular first.
    pub async fn search(&self, query: &str, media: Option<MediaType>) -> Result<Vec<DiscoverCard>> {
        let t = self.tmdb()?;
        let mut all: Vec<(MediaType, SearchResult)> = vec![];
        if media != Some(MediaType::Tv) {
            all.extend(
                t.search_movie(query, None)
                    .await?
                    .results
                    .into_iter()
                    .map(|r| (MediaType::Movie, r)),
            );
        }
        if media != Some(MediaType::Movie) {
            all.extend(
                t.search_tv(query, None)
                    .await?
                    .results
                    .into_iter()
                    .map(|r| (MediaType::Tv, r)),
            );
        }
        all.sort_by(|a, b| {
            b.1.popularity
                .partial_cmp(&a.1.popularity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut results = vec![];
        for (mt, mut r) in all {
            r.media_type = Some(mt.as_str().to_string());
            results.push(r);
        }
        self.cards(results, None).await
    }

    pub async fn detail(&self, media: MediaType, tmdb_id: i64) -> Result<DiscoverDetail> {
        let t = self.tmdb()?;
        let (mut sr, genres, runtime, imdb, tvdb, seasons) = match media {
            MediaType::Movie => {
                let m = t.movie(tmdb_id).await?;
                (
                    SearchResult {
                        id: m.id,
                        title: Some(m.title.clone()),
                        release_date: m.release_date.clone(),
                        overview: m.overview.clone(),
                        poster_path: m.poster_path.clone(),
                        backdrop_path: m.backdrop_path.clone(),
                        vote_average: m.vote_average,
                        ..Default::default()
                    },
                    m.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>(),
                    m.runtime,
                    m.imdb_id.clone(),
                    None,
                    vec![],
                )
            }
            MediaType::Tv => {
                let m = t.tv(tmdb_id).await?;
                let ext = m.external_ids.clone();
                (
                    SearchResult {
                        id: m.id,
                        name: Some(m.name.clone()),
                        first_air_date: m.first_air_date.clone(),
                        overview: m.overview.clone(),
                        poster_path: m.poster_path.clone(),
                        backdrop_path: m.backdrop_path.clone(),
                        vote_average: m.vote_average,
                        ..Default::default()
                    },
                    m.genres.iter().map(|g| g.name.clone()).collect(),
                    m.episode_run_time.first().copied(),
                    ext.as_ref().and_then(|e| e.imdb_id.clone()),
                    ext.and_then(|e| e.tvdb_id),
                    m.seasons
                        .iter()
                        .map(|s| s.season_number)
                        .filter(|n| *n > 0)
                        .collect(),
                )
            }
        };
        sr.media_type = Some(media.as_str().to_string());
        let card = self
            .cards(vec![sr], Some(media))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("tmdb {tmdb_id}")))?;
        Ok(DiscoverDetail {
            card,
            genres,
            runtime_minutes: runtime,
            imdb_id: imdb,
            tvdb_id: tvdb,
            seasons,
        })
    }
}

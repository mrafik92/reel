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
    /// Library item (movie, or show) holding this title, if it is in the library.
    pub library_item_id: Option<i64>,
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
    /// Top-billed cast (max 20).
    pub cast: Vec<DiscoverCast>,
}

/// Cast entry of a discover detail.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoverCast {
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    pub person_tmdb_id: i64,
}

/// `GET /people/{tmdb_id}`.
#[derive(Debug, Clone, Serialize)]
pub struct PersonDetail {
    pub tmdb_id: i64,
    pub name: String,
    pub biography: Option<String>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    pub known_for_department: Option<String>,
    pub profile_path: Option<String>,
    pub imdb_id: Option<String>,
    pub credits: Vec<PersonCredit>,
}

/// One filmography entry: a discover card plus the person's part in it.
#[derive(Debug, Clone, Serialize)]
pub struct PersonCredit {
    pub card: DiscoverCard,
    /// `cast` | `crew`
    pub role: &'static str,
    pub character: Option<String>,
    pub job: Option<String>,
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
        let (movies_ids, tv_ids) = (movies.clone(), tv.clone());
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
        let mut lib_ids: HashMap<(MediaType, i64), i64> = HashMap::new();
        for (mt, ids) in [(MediaType::Movie, &movies_ids), (MediaType::Tv, &tv_ids)] {
            for (id, item) in crate::service::library_item_ids(&self.s.pool, mt, ids).await? {
                lib_ids.insert((mt, id), item);
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
                library_item_id: lib_ids.get(&(mt, r.id)).copied(),
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
        let (mut sr, genres, runtime, imdb, tvdb, seasons, credits) = match media {
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
                    m.credits.clone(),
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
                    m.credits.clone(),
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
            cast: credits
                .map(|c| {
                    let mut cast = c.cast;
                    cast.sort_by_key(|m| m.order);
                    cast.into_iter()
                        .take(20)
                        .map(|m| DiscoverCast {
                            name: m.name,
                            character: m.character.filter(|c| !c.is_empty()),
                            profile_path: m.profile_path,
                            person_tmdb_id: m.id,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Person page: biography plus filmography as discover cards (availability, progress).
    pub async fn person(&self, tmdb_id: i64) -> Result<PersonDetail> {
        let p = self.tmdb()?.person(tmdb_id).await?;
        let mut seen: HashMap<(MediaType, i64), usize> = HashMap::new();
        let mut entries: Vec<(SearchResult, &'static str, Option<String>, Option<String>)> = vec![];
        let all = p.combined_credits.iter().flat_map(|c| {
            c.cast
                .iter()
                .map(|x| (x, "cast"))
                .chain(c.crew.iter().map(|x| (x, "crew")))
        });
        for (c, role) in all {
            let mt = match c.media_type.as_deref().and_then(MediaType::parse) {
                Some(m) => m,
                None => continue,
            };
            let key = (mt, c.id);
            let job = c.job.clone().filter(|j| !j.is_empty());
            let character = c.character.clone().filter(|j| !j.is_empty());
            if let Some(&i) = seen.get(&key) {
                // cast wins over crew; several crew jobs on one title are merged
                let e = &mut entries[i];
                if e.1 == "crew" && role == "crew" {
                    if let Some(j) = job {
                        match &mut e.3 {
                            Some(old) if !old.split(", ").any(|x| x == j) => {
                                old.push_str(", ");
                                old.push_str(&j);
                            }
                            None => e.3 = Some(j),
                            _ => {}
                        }
                    }
                }
                continue;
            }
            let year_known = c.date().is_some();
            if c.poster_path.is_none() && !year_known {
                continue;
            }
            seen.insert(key, entries.len());
            let sr = SearchResult {
                id: c.id,
                title: c.title.clone(),
                name: c.name.clone(),
                release_date: c.release_date.clone().filter(|d| !d.is_empty()),
                first_air_date: c.first_air_date.clone().filter(|d| !d.is_empty()),
                overview: c.overview.clone(),
                poster_path: c.poster_path.clone(),
                backdrop_path: c.backdrop_path.clone(),
                vote_average: c.vote_average,
                media_type: Some(mt.as_str().to_string()),
                ..Default::default()
            };
            entries.push((sr, role, character, job));
        }
        // newest first; undated entries last
        entries.sort_by(|a, b| {
            let d = |e: &(SearchResult, _, _, _)| {
                e.0.release_date
                    .clone()
                    .or_else(|| e.0.first_air_date.clone())
                    .unwrap_or_default()
            };
            d(b).cmp(&d(a))
        });
        let cards = self
            .cards(entries.iter().map(|e| e.0.clone()).collect(), None)
            .await?;
        let credits = cards
            .into_iter()
            .zip(entries)
            .map(|(card, (_, role, character, job))| PersonCredit {
                card,
                role,
                character: if role == "cast" { character } else { None },
                job: if role == "crew" { job } else { None },
            })
            .collect();
        Ok(PersonDetail {
            tmdb_id: p.id,
            name: p.name,
            biography: p.biography.filter(|b| !b.trim().is_empty()),
            birthday: p.birthday.filter(|b| !b.is_empty()),
            deathday: p.deathday.filter(|b| !b.is_empty()),
            place_of_birth: p.place_of_birth.filter(|b| !b.is_empty()),
            known_for_department: p.known_for_department.filter(|b| !b.is_empty()),
            profile_path: p.profile_path,
            imdb_id: p
                .imdb_id
                .or_else(|| p.external_ids.and_then(|e| e.imdb_id))
                .filter(|i| !i.is_empty()),
            credits,
        })
    }
}

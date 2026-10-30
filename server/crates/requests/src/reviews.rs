//! Ratings and reviews for a title: TMDB votes + reviews, optional OMDb scores (IMDb, Rotten
//! Tomatoes, Metacritic) and optional Trakt comments. Official APIs only — nothing is scraped.

use std::time::Duration;

use reel_metadata::TmdbClient;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{Error, MediaType, Result};

pub const OMDB_BASE: &str = "https://www.omdbapi.com";
pub const TRAKT_BASE: &str = "https://api.trakt.tv";
const OMDB_TTL_SECS: i64 = 24 * 3600;
const MAX_REVIEWS: usize = 10;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Rating {
    /// `imdb` | `rotten_tomatoes` | `metacritic` | `tmdb`
    pub source: &'static str,
    pub label: String,
    /// Display value: `7.8/10`, `94%`.
    pub value: String,
    pub score_100: Option<f64>,
    pub votes: Option<i64>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Review {
    /// `tmdb` | `trakt`
    pub source: &'static str,
    pub author: String,
    /// TMDB avatar path (`/x.jpg`); the API layer turns it into a signed image URL.
    pub avatar_path: Option<String>,
    /// 0..10
    pub rating: Option<f64>,
    pub content: String,
    pub created_at: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExternalLink {
    /// `imdb` | `rotten_tomatoes` | `letterboxd` | `tmdb` | `trakt`
    pub source: &'static str,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ReviewBundle {
    pub ratings: Vec<Rating>,
    pub reviews: Vec<Review>,
    pub links: Vec<ExternalLink>,
}

/// What we keep from an OMDb response.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OmdbScores {
    pub imdb_rating: Option<f64>,
    pub imdb_votes: Option<i64>,
    pub rotten_tomatoes: Option<f64>,
    pub metascore: Option<f64>,
    pub tomato_url: Option<String>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("reel/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client")
}

fn na(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty() && *s != "N/A")
}

/// OMDb client.
#[derive(Debug, Clone)]
pub struct OmdbClient {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl OmdbClient {
    pub fn new(key: impl Into<String>) -> Self {
        Self::with_base_url(key, OMDB_BASE)
    }
    pub fn with_base_url(key: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            http: http(),
            base: base.into().trim_end_matches('/').to_string(),
            key: key.into(),
        }
    }

    async fn fetch(&self, imdb_id: &str) -> Result<Option<OmdbScores>> {
        let resp = self
            .http
            .get(format!("{}/", self.base))
            .query(&[("i", imdb_id), ("apikey", &self.key), ("tomatoes", "true")])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: String::new(),
            });
        }
        let v: serde_json::Value = resp.json().await?;
        if v["Response"].as_str() == Some("False") {
            return Ok(None);
        }
        let s = |k: &str| na(v[k].as_str()).map(str::to_string);
        let mut out = OmdbScores {
            imdb_rating: s("imdbRating").and_then(|r| r.parse().ok()),
            imdb_votes: s("imdbVotes").and_then(|r| r.replace(',', "").parse().ok()),
            metascore: s("Metascore").and_then(|r| r.parse().ok()),
            tomato_url: s("tomatoURL").filter(|u| u.starts_with("http")),
            ..Default::default()
        };
        if let Some(rs) = v["Ratings"].as_array() {
            for r in rs {
                if r["Source"].as_str() == Some("Rotten Tomatoes") {
                    out.rotten_tomatoes = r["Value"]
                        .as_str()
                        .and_then(|x| x.trim_end_matches('%').parse().ok());
                }
            }
        }
        Ok(Some(out))
    }
}

/// Trakt client.
#[derive(Debug, Clone)]
pub struct TraktClient {
    http: reqwest::Client,
    base: String,
    client_id: String,
}

struct TraktTitle {
    slug: String,
    comments: Vec<Review>,
}

impl TraktClient {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self::with_base_url(client_id, TRAKT_BASE)
    }
    pub fn with_base_url(client_id: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            http: http(),
            base: base.into().trim_end_matches('/').to_string(),
            client_id: client_id.into(),
        }
    }

    async fn get(&self, path: &str, q: &[(&str, String)]) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(format!("{}{path}", self.base))
            .query(q)
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.client_id)
            .header("Content-Type", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: String::new(),
            });
        }
        Ok(resp.json().await?)
    }

    async fn title(&self, media: MediaType, tmdb_id: i64) -> Result<Option<TraktTitle>> {
        let (ty, plural) = match media {
            MediaType::Movie => ("movie", "movies"),
            MediaType::Tv => ("show", "shows"),
        };
        let found = self
            .get(&format!("/search/tmdb/{tmdb_id}"), &[("type", ty.into())])
            .await?;
        let Some(slug) = found
            .as_array()
            .and_then(|a| a.iter().find_map(|r| r[ty]["ids"]["slug"].as_str()))
            .map(str::to_string)
        else {
            return Ok(None);
        };
        let comments = self
            .get(
                &format!("/{plural}/{slug}/comments/likes"),
                &[("limit", MAX_REVIEWS.to_string())],
            )
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|c| {
                let content = c["comment"].as_str()?.trim().to_string();
                if content.is_empty() {
                    return None;
                }
                let user = &c["user"];
                Some(Review {
                    source: "trakt",
                    author: user["name"]
                        .as_str()
                        .filter(|n| !n.is_empty())
                        .or(user["username"].as_str())
                        .unwrap_or("Trakt user")
                        .to_string(),
                    avatar_path: None,
                    rating: c["user_rating"].as_f64(),
                    content,
                    created_at: c["created_at"].as_str().unwrap_or_default().to_string(),
                    url: c["id"]
                        .as_i64()
                        .map(|id| format!("https://trakt.tv/comments/{id}")),
                })
            })
            .collect();
        Ok(Some(TraktTitle { slug, comments }))
    }
}

/// Aggregates the sources.
#[derive(Clone)]
pub struct ReviewsService {
    pool: SqlitePool,
    tmdb: TmdbClient,
    omdb: Option<OmdbClient>,
    trakt: Option<TraktClient>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl ReviewsService {
    pub fn new(
        pool: SqlitePool,
        tmdb: TmdbClient,
        omdb: Option<OmdbClient>,
        trakt: Option<TraktClient>,
    ) -> Self {
        Self {
            pool,
            tmdb,
            omdb,
            trakt,
        }
    }

    /// OMDb scores via the 24 h SQLite cache. Failures are silent (`None`).
    async fn omdb_scores(&self, imdb_id: &str) -> Option<OmdbScores> {
        let omdb = self.omdb.as_ref()?;
        let cached: Option<(String, i64)> =
            sqlx::query_as("SELECT json, fetched_at FROM omdb_cache WHERE imdb_id = ?")
                .bind(imdb_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        if let Some((j, at)) = &cached {
            if now() - at < OMDB_TTL_SECS {
                if let Ok(s) = serde_json::from_str(j) {
                    return Some(s);
                }
            }
        }
        match omdb.fetch(imdb_id).await {
            Ok(Some(s)) => {
                let _ = sqlx::query(
                    "INSERT INTO omdb_cache (imdb_id, json, fetched_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(imdb_id) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
                )
                .bind(imdb_id)
                .bind(serde_json::to_string(&s).unwrap_or_default())
                .bind(now())
                .execute(&self.pool)
                .await;
                Some(s)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(imdb_id, error = %e, "OMDb lookup failed");
                // stale data beats none
                cached.and_then(|(j, _)| serde_json::from_str(&j).ok())
            }
        }
    }

    pub async fn get(&self, media: MediaType, tmdb_id: i64) -> Result<ReviewBundle> {
        let tv = media == MediaType::Tv;
        let (title, vote_avg, vote_count, imdb_id) = if tv {
            let d = self.tmdb.tv_cached(tmdb_id).await?;
            let imdb = d.external_ids.and_then(|e| e.imdb_id);
            (d.name, d.vote_average, d.vote_count, imdb)
        } else {
            let d = self.tmdb.movie_cached(tmdb_id).await?;
            (d.title, d.vote_average, d.vote_count, d.imdb_id)
        };
        let imdb_id = imdb_id.filter(|i| !i.is_empty());
        let mut out = ReviewBundle::default();

        let omdb = match &imdb_id {
            Some(i) => self.omdb_scores(i).await,
            None => None,
        };
        let imdb_url = imdb_id
            .as_deref()
            .map(|i| format!("https://www.imdb.com/title/{i}/reviews"));
        let tmdb_url = format!("https://www.themoviedb.org/{}/{tmdb_id}", media.as_str());
        if let Some(r) = omdb.as_ref().and_then(|o| o.imdb_rating) {
            out.ratings.push(Rating {
                source: "imdb",
                label: "IMDb".into(),
                value: format!("{r:.1}/10"),
                score_100: Some((r * 10.0).round()),
                votes: omdb.as_ref().and_then(|o| o.imdb_votes),
                url: imdb_url.clone(),
            });
        }
        let rt_url = omdb
            .as_ref()
            .and_then(|o| o.tomato_url.clone())
            .unwrap_or_else(|| {
                reqwest::Url::parse_with_params(
                    "https://www.rottentomatoes.com/search",
                    &[("search", title.as_str())],
                )
                .map_or_else(|_| "https://www.rottentomatoes.com".into(), Into::into)
            });
        if let Some(r) = omdb.as_ref().and_then(|o| o.rotten_tomatoes) {
            out.ratings.push(Rating {
                source: "rotten_tomatoes",
                label: "Rotten Tomatoes".into(),
                value: format!("{}%", r.round()),
                score_100: Some(r),
                votes: None,
                url: Some(rt_url.clone()),
            });
        }
        if let Some(m) = omdb.as_ref().and_then(|o| o.metascore) {
            out.ratings.push(Rating {
                source: "metacritic",
                label: "Metacritic".into(),
                value: format!("{}/100", m.round()),
                score_100: Some(m),
                votes: None,
                url: None,
            });
        }
        if let Some(v) = vote_avg.filter(|v| *v > 0.0) {
            out.ratings.push(Rating {
                source: "tmdb",
                label: "TMDB".into(),
                value: format!("{v:.1}/10"),
                score_100: Some((v * 10.0).round()),
                votes: vote_count,
                url: Some(tmdb_url.clone()),
            });
        }

        // TMDB reviews: newest first, up to 10.
        match self.tmdb.reviews(tv, tmdb_id).await {
            Ok(page) => {
                let mut rs: Vec<Review> = page
                    .results
                    .into_iter()
                    .filter(|r| !r.content.trim().is_empty())
                    .map(|r| {
                        let d = r.author_details.unwrap_or_default();
                        Review {
                            source: "tmdb",
                            author: r.author,
                            avatar_path: d
                                .avatar_path
                                .filter(|p| p.starts_with('/') && !p.starts_with("/http")),
                            rating: d.rating,
                            content: r.content.trim().to_string(),
                            created_at: r.created_at,
                            url: r.url,
                        }
                    })
                    .collect();
                rs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                rs.truncate(MAX_REVIEWS);
                out.reviews = rs;
            }
            Err(e) => tracing::warn!(tmdb_id, error = %e, "TMDB reviews failed"),
        }

        let mut trakt_link = None;
        if let Some(t) = &self.trakt {
            match t.title(media, tmdb_id).await {
                Ok(Some(tt)) => {
                    trakt_link = Some(format!(
                        "https://trakt.tv/{}/{}",
                        if tv { "shows" } else { "movies" },
                        tt.slug
                    ));
                    out.reviews.extend(tt.comments);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(tmdb_id, error = %e, "Trakt lookup failed"),
            }
        }

        if let Some(u) = imdb_url {
            out.links.push(ExternalLink {
                source: "imdb",
                url: u,
            });
        }
        out.links.push(ExternalLink {
            source: "rotten_tomatoes",
            url: rt_url,
        });
        if !tv {
            out.links.push(ExternalLink {
                source: "letterboxd",
                url: format!("https://letterboxd.com/tmdb/{tmdb_id}"),
            });
        }
        out.links.push(ExternalLink {
            source: "tmdb",
            url: tmdb_url,
        });
        if let Some(u) = trakt_link {
            out.links.push(ExternalLink {
                source: "trakt",
                url: u,
            });
        }
        Ok(out)
    }
}

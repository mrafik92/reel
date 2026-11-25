//! Minimal typed TMDB v3 client (API key or v4 bearer token).

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;

use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::Semaphore;

use crate::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.themoviedb.org/3";
const MAX_RETRIES: u32 = 4;

#[derive(Debug, Clone)]
enum Auth {
    ApiKey(String),
    Bearer(String),
}

/// TMDB client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct TmdbClient {
    http: reqwest::Client,
    base: String,
    auth: Auth,
    gate: Arc<Semaphore>,
    /// Short-lived response cache (person pages, reviews, details), shared between clones.
    cache: Arc<DashMap<String, (Instant, serde_json::Value)>>,
}

/// How long cached TMDB responses live.
const CACHE_TTL: Duration = Duration::from_secs(3600);

// ---- models (only fields we use) ----

#[derive(Debug, Clone, Deserialize)]
pub struct Paged<T> {
    #[serde(default)]
    pub page: u32,
    pub results: Vec<T>,
    #[serde(default)]
    pub total_pages: u32,
    #[serde(default)]
    pub total_results: u32,
}

/// Search/trending/discover item for both movies and TV.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchResult {
    pub id: i64,
    pub title: Option<String>,
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub original_name: Option<String>,
    pub release_date: Option<String>,
    pub first_air_date: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub popularity: Option<f64>,
    pub vote_average: Option<f64>,
    /// Present on multi-type endpoints (trending/all).
    pub media_type: Option<String>,
}

impl SearchResult {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().or(self.name.as_deref()).unwrap_or("")
    }
    pub fn original_display_title(&self) -> Option<&str> {
        self.original_title
            .as_deref()
            .or(self.original_name.as_deref())
    }
    pub fn year(&self) -> Option<i32> {
        year_of(
            self.release_date
                .as_deref()
                .or(self.first_air_date.as_deref()),
        )
    }
}

/// Parse the year out of a `YYYY-MM-DD` string.
pub fn year_of(date: Option<&str>) -> Option<i32> {
    date?.get(..4)?.parse().ok()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CastMember {
    pub id: i64,
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    #[serde(default)]
    pub order: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CrewMember {
    pub id: i64,
    pub name: String,
    pub job: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastMember>,
    #[serde(default)]
    pub crew: Vec<CrewMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReleaseDates {
    #[serde(default)]
    pub results: Vec<ReleaseDatesCountry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDatesCountry {
    pub iso_3166_1: String,
    #[serde(default)]
    pub release_dates: Vec<ReleaseDateEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDateEntry {
    #[serde(default)]
    pub certification: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContentRatings {
    #[serde(default)]
    pub results: Vec<ContentRatingEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentRatingEntry {
    pub iso_3166_1: String,
    #[serde(default)]
    pub rating: String,
}

/// `append_to_response=videos`: trailers, teasers and clips, mostly on YouTube.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Videos {
    #[serde(default)]
    pub results: Vec<Video>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Video {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub site: String,
    /// `Trailer`, `Teaser`, `Clip`, `Featurette`, …
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub official: bool,
    #[serde(default)]
    pub published_at: String,
}

/// A plausible YouTube video id (11 characters of `[A-Za-z0-9_-]`). Anything else is never
/// handed to the client, which builds an embed URL from it.
pub fn is_youtube_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Videos {
    /// The YouTube id of the title's main trailer: official trailers first, then any trailer,
    /// then teasers; the earliest published among equals (the first trailer, or season 1's).
    pub fn youtube_trailer(&self) -> Option<String> {
        self.results
            .iter()
            .filter(|v| v.site.eq_ignore_ascii_case("YouTube") && is_youtube_id(&v.key))
            .filter_map(|v| {
                let rank = match (v.kind.as_str(), v.official) {
                    ("Trailer", true) => 0,
                    ("Trailer", false) => 1,
                    ("Teaser", true) => 2,
                    ("Teaser", false) => 3,
                    _ => return None,
                };
                Some((rank, v.published_at.as_str(), v))
            })
            .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
            .map(|(_, _, v)| v.key.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieDetails {
    pub id: i64,
    pub title: String,
    pub release_date: Option<String>,
    pub overview: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub original_language: Option<String>,
    /// Minutes.
    pub runtime: Option<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    pub belongs_to_collection: Option<CollectionRef>,
    pub credits: Option<Credits>,
    pub release_dates: Option<ReleaseDates>,
    pub external_ids: Option<ExternalIds>,
    /// Only on [`TmdbClient::movie`] (the enrichment fetch).
    pub videos: Option<Videos>,
}

impl MovieDetails {
    /// US certification, if any.
    pub fn us_certification(&self) -> Option<String> {
        self.release_dates
            .as_ref()?
            .results
            .iter()
            .find(|c| c.iso_3166_1 == "US")?
            .release_dates
            .iter()
            .map(|r| r.certification.trim())
            .find(|c| !c.is_empty())
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonRef {
    pub season_number: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TvDetails {
    pub id: i64,
    pub name: String,
    pub first_air_date: Option<String>,
    pub overview: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub original_language: Option<String>,
    #[serde(default)]
    pub episode_run_time: Vec<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub seasons: Vec<SeasonRef>,
    pub credits: Option<Credits>,
    pub external_ids: Option<ExternalIds>,
    pub content_ratings: Option<ContentRatings>,
    /// Only on [`TmdbClient::tv`] (the enrichment fetch).
    pub videos: Option<Videos>,
}

impl TvDetails {
    pub fn us_rating(&self) -> Option<String> {
        self.content_ratings
            .as_ref()?
            .results
            .iter()
            .find(|c| c.iso_3166_1 == "US" && !c.rating.is_empty())
            .map(|c| c.rating.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeDetails {
    pub episode_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub still_path: Option<String>,
    /// Minutes.
    pub runtime: Option<i64>,
    pub vote_average: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonDetails {
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub poster_path: Option<String>,
    #[serde(default)]
    pub episodes: Vec<EpisodeDetails>,
}

/// `/person/{id}` with combined credits.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonDetails {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub biography: Option<String>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    pub known_for_department: Option<String>,
    pub profile_path: Option<String>,
    pub imdb_id: Option<String>,
    pub external_ids: Option<ExternalIds>,
    pub combined_credits: Option<CombinedCredits>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CombinedCredits {
    #[serde(default)]
    pub cast: Vec<CombinedCredit>,
    #[serde(default)]
    pub crew: Vec<CombinedCredit>,
}

/// One cast or crew entry of a person's filmography (movie or TV).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CombinedCredit {
    pub id: i64,
    /// `movie` | `tv`
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub first_air_date: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub character: Option<String>,
    pub job: Option<String>,
}

impl CombinedCredit {
    pub fn date(&self) -> Option<&str> {
        self.release_date
            .as_deref()
            .or(self.first_air_date.as_deref())
            .filter(|d| !d.is_empty())
    }
}

/// Entry of `/search/person`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonResult {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub profile_path: Option<String>,
    pub known_for_department: Option<String>,
    #[serde(default)]
    pub known_for: Vec<SearchResult>,
}

/// `/{movie|tv}/{id}/reviews`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReviewsPage {
    #[serde(default)]
    pub results: Vec<TmdbReview>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TmdbReview {
    #[serde(default)]
    pub author: String,
    pub author_details: Option<AuthorDetails>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub created_at: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorDetails {
    pub avatar_path: Option<String>,
    pub rating: Option<f64>,
}

impl TmdbClient {
    /// Create a client against the public TMDB API. A key starting with `eyJ`
    /// (JWT) is treated as a v4 bearer token, otherwise as a v3 API key.
    pub fn new(key: impl Into<String>) -> Self {
        Self::with_base_url(key, DEFAULT_BASE_URL)
    }

    /// Build from `REEL_TMDB_API_KEY`, if set and non-empty.
    pub fn from_env() -> Option<Self> {
        std::env::var("REEL_TMDB_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .map(Self::new)
    }

    pub fn with_base_url(key: impl Into<String>, base: impl Into<String>) -> Self {
        let key = key.into().trim().to_string();
        let auth = if key.starts_with("eyJ") {
            Auth::Bearer(key)
        } else {
            Auth::ApiKey(key)
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("reel/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            auth,
            gate: Arc::new(Semaphore::new(8)),
            cache: Arc::default(),
        }
    }

    /// Like [`Self::get`] but memoised for an hour (keyed by path + params).
    async fn get_cached<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        self.get_cached_for(path, params, CACHE_TTL).await
    }

    /// Memoised GET with an explicit lifetime (entries are dropped lazily).
    async fn get_cached_for<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
        ttl: Duration,
    ) -> Result<T> {
        let key = format!("{path}?{params:?}");
        if let Some(hit) = self.cache.get(&key) {
            if hit.0.elapsed() < ttl {
                if let Ok(v) = serde_json::from_value(hit.1.clone()) {
                    return Ok(v);
                }
            }
        }
        let v: serde_json::Value = self.get(path, params).await?;
        if self.cache.len() > 2000 {
            self.cache
                .retain(|_, (t, _)| t.elapsed() < Duration::from_secs(86_400));
        }
        self.cache.insert(key, (Instant::now(), v.clone()));
        serde_json::from_value(v)
            .map_err(|e| Error::Other(format!("unexpected TMDB response: {e}")))
    }

    /// Person with biography, external ids and combined credits (cached 1 h).
    pub async fn person(&self, id: i64) -> Result<PersonDetails> {
        self.get_cached(
            &format!("/person/{id}"),
            &[("append_to_response", "combined_credits,external_ids".into())],
        )
        .await
    }

    /// `/{movie|tv}/{id}/recommendations`, cached 24 h.
    pub async fn recommendations(&self, tv: bool, id: i64) -> Result<Paged<SearchResult>> {
        let kind = if tv { "tv" } else { "movie" };
        self.get_cached_for(
            &format!("/{kind}/{id}/recommendations"),
            &[],
            Duration::from_secs(86_400),
        )
        .await
    }

    /// `/{movie|tv}/{id}/similar`, cached 24 h.
    pub async fn similar(&self, tv: bool, id: i64) -> Result<Paged<SearchResult>> {
        let kind = if tv { "tv" } else { "movie" };
        self.get_cached_for(
            &format!("/{kind}/{id}/similar"),
            &[],
            Duration::from_secs(86_400),
        )
        .await
    }

    /// Search movies (`tv = false`) or TV, cached 10 min (typing-friendly).
    pub async fn search_cached(&self, tv: bool, query: &str) -> Result<Paged<SearchResult>> {
        self.get_cached_for(
            if tv { "/search/tv" } else { "/search/movie" },
            &[("query", query.to_string())],
            Duration::from_secs(600),
        )
        .await
    }

    /// People search, cached 10 min.
    pub async fn search_person(&self, query: &str) -> Result<Paged<PersonResult>> {
        self.get_cached_for(
            "/search/person",
            &[("query", query.to_string())],
            Duration::from_secs(600),
        )
        .await
    }

    /// First page of user reviews (cached 1 h).
    pub async fn reviews(&self, tv: bool, id: i64) -> Result<ReviewsPage> {
        let kind = if tv { "tv" } else { "movie" };
        self.get_cached(&format!("/{kind}/{id}/reviews"), &[]).await
    }

    /// `/{movie|tv}/{id}/videos` (uncached; the trailer backfill asks once per title).
    pub async fn videos(&self, tv: bool, id: i64) -> Result<Videos> {
        let kind = if tv { "tv" } else { "movie" };
        self.get(&format!("/{kind}/{id}/videos"), &[]).await
    }

    /// [`Self::movie`], cached 1 h (for read-only display paths).
    pub async fn movie_cached(&self, id: i64) -> Result<MovieDetails> {
        self.get_cached(
            &format!("/movie/{id}"),
            &[(
                "append_to_response",
                "credits,release_dates,external_ids".into(),
            )],
        )
        .await
    }

    /// [`Self::tv`], cached 1 h.
    pub async fn tv_cached(&self, id: i64) -> Result<TvDetails> {
        self.get_cached(
            &format!("/tv/{id}"),
            &[(
                "append_to_response",
                "external_ids,credits,content_ratings".into(),
            )],
        )
        .await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            let _permit = self.gate.acquire().await.expect("semaphore open");
            let mut req = self.http.get(&url).query(params);
            req = match &self.auth {
                Auth::ApiKey(k) => req.query(&[("api_key", k)]),
                Auth::Bearer(t) => req.bearer_auth(t),
            };
            let resp = req.send().await?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < MAX_RETRIES {
                attempt += 1;
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1)
                    .min(30);
                drop(_permit);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::Status(status.as_u16()));
            }
            return Ok(resp.json().await?);
        }
    }

    pub async fn search_movie(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<Paged<SearchResult>> {
        let mut p = vec![("query", query.to_string())];
        if let Some(y) = year {
            p.push(("year", y.to_string()));
        }
        self.get("/search/movie", &p).await
    }

    pub async fn search_tv(&self, query: &str, year: Option<i32>) -> Result<Paged<SearchResult>> {
        let mut p = vec![("query", query.to_string())];
        if let Some(y) = year {
            p.push(("first_air_date_year", y.to_string()));
        }
        self.get("/search/tv", &p).await
    }

    /// Full details for enrichment, with credits, certifications, external ids and videos.
    pub async fn movie(&self, id: i64) -> Result<MovieDetails> {
        self.get(
            &format!("/movie/{id}"),
            &[(
                "append_to_response",
                "credits,release_dates,external_ids,videos".into(),
            )],
        )
        .await
    }

    /// Full details for enrichment, with external ids, credits, ratings and videos.
    pub async fn tv(&self, id: i64) -> Result<TvDetails> {
        self.get(
            &format!("/tv/{id}"),
            &[(
                "append_to_response",
                "external_ids,credits,content_ratings,videos".into(),
            )],
        )
        .await
    }

    pub async fn season(&self, tv_id: i64, season: i64) -> Result<SeasonDetails> {
        self.get(&format!("/tv/{tv_id}/season/{season}"), &[]).await
    }

    /// `media` is `movie`, `tv` or `all`; `window` is `day` or `week`.
    pub async fn trending(
        &self,
        media: &str,
        window: &str,
        page: u32,
    ) -> Result<Paged<SearchResult>> {
        self.get(
            &format!("/trending/{media}/{window}"),
            &[("page", page.to_string())],
        )
        .await
    }

    pub async fn popular_movies(&self, page: u32) -> Result<Paged<SearchResult>> {
        self.get("/movie/popular", &[("page", page.to_string())])
            .await
    }

    pub async fn popular_tv(&self, page: u32) -> Result<Paged<SearchResult>> {
        self.get("/tv/popular", &[("page", page.to_string())]).await
    }

    pub async fn upcoming_movies(&self, page: u32) -> Result<Paged<SearchResult>> {
        self.get("/movie/upcoming", &[("page", page.to_string())])
            .await
    }

    /// Discover with arbitrary TMDB filter params; `tv` selects `/discover/tv`.
    pub async fn discover(
        &self,
        tv: bool,
        params: &[(&str, String)],
    ) -> Result<Paged<SearchResult>> {
        self.get(
            if tv {
                "/discover/tv"
            } else {
                "/discover/movie"
            },
            params,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param},
    };

    #[tokio::test]
    async fn search_movie_sends_key_and_parses() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/movie"))
            .and(query_param("api_key", "k"))
            .and(query_param("query", "Dune"))
            .and(query_param("year", "2021"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "page":1,"total_pages":1,"total_results":1,
                "results":[{"id":438631,"title":"Dune","release_date":"2021-09-15","popularity":99.0}]
            })))
            .mount(&s)
            .await;
        let c = TmdbClient::with_base_url("k", s.uri());
        let r = c.search_movie("Dune", Some(2021)).await.unwrap();
        assert_eq!(r.results[0].id, 438631);
        assert_eq!(r.results[0].year(), Some(2021));
    }

    #[tokio::test]
    async fn bearer_token_used() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/popular"))
            .and(header("authorization", "Bearer eyJabc"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"results":[]})),
            )
            .mount(&s)
            .await;
        let c = TmdbClient::with_base_url("eyJabc", s.uri());
        assert!(c.popular_movies(1).await.unwrap().results.is_empty());
    }

    #[tokio::test]
    async fn retries_on_429() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/popular"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/tv/popular"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"results":[{"id":1,"name":"X"}]})),
            )
            .mount(&s)
            .await;
        let c = TmdbClient::with_base_url("k", s.uri());
        assert_eq!(c.popular_tv(1).await.unwrap().results.len(), 1);
    }

    #[tokio::test]
    async fn movie_details_append_and_cert() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/1"))
            .and(query_param("append_to_response", "credits,release_dates,external_ids,videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":1,"title":"T","genres":[{"id":1,"name":"Drama"}],
                "release_dates":{"results":[
                    {"iso_3166_1":"GB","release_dates":[{"certification":"15"}]},
                    {"iso_3166_1":"US","release_dates":[{"certification":""},{"certification":"PG-13"}]}]},
                "videos":{"results":[
                    {"key":"teaserAAAAA","site":"YouTube","type":"Teaser","official":true,"published_at":"2020-01-01"},
                    {"key":"trailer2BBB","site":"YouTube","type":"Trailer","official":true,"published_at":"2020-06-01"},
                    {"key":"trailer1CCC","site":"YouTube","type":"Trailer","official":true,"published_at":"2020-03-01"},
                    {"key":"fanmadeDDDD","site":"YouTube","type":"Trailer","official":false,"published_at":"2019-01-01"},
                    {"key":"vimeo","site":"Vimeo","type":"Trailer","official":true,"published_at":"2019-01-01"}]}
            })))
            .mount(&s)
            .await;
        let c = TmdbClient::with_base_url("k", s.uri());
        let m = c.movie(1).await.unwrap();
        assert_eq!(m.us_certification().as_deref(), Some("PG-13"));
        assert_eq!(
            m.videos.unwrap().youtube_trailer().as_deref(),
            Some("trailer1CCC")
        );
    }

    #[test]
    fn trailer_falls_back_to_teaser_and_skips_bad_ids() {
        let v: Videos = serde_json::from_value(serde_json::json!({"results":[
            {"key":"clipAAAAAAA","site":"YouTube","type":"Clip","official":true},
            {"key":"bad id/../x","site":"YouTube","type":"Trailer","official":true},
            {"key":"teaserBBBBB","site":"YouTube","type":"Teaser","official":false}
        ]}))
        .unwrap();
        assert_eq!(v.youtube_trailer().as_deref(), Some("teaserBBBBB"));
        assert_eq!(Videos::default().youtube_trailer(), None);
        assert!(is_youtube_id("aqz-KE-bpKQ"));
        assert!(!is_youtube_id("aqz-KE-bpK"));
    }

    #[tokio::test]
    async fn error_status_surfaces() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        let c = TmdbClient::with_base_url("k", s.uri());
        assert!(matches!(c.movie(9).await, Err(Error::Status(404))));
    }
}

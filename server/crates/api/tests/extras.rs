mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{Limits, NewUser, Role};
use reel_metadata::TmdbClient;
use reel_requests::reviews::{OmdbClient, TraktClient};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

struct Env {
    state: AppState,
    admin: Router,
    _dir: tempfile::TempDir,
}

async fn env(tmdb: &MockServer, omdb: Option<&MockServer>, trakt: Option<&MockServer>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let state = AppState::new(db, cfg)
        .with_tmdb(TmdbClient::with_base_url("k", tmdb.uri()))
        .with_review_sources(
            omdb.map(|s| OmdbClient::with_base_url("okey", s.uri())),
            trakt.map(|s| TraktClient::with_base_url("tid", s.uri())),
        );
    let admin = common::authed(&state, app(state.clone())).await;
    Env {
        state,
        admin,
        _dir: dir,
    }
}

async fn user_app(e: &Env, name: &str, libs: Vec<i64>) -> (Router, i64) {
    let u = e
        .state
        .auth
        .create_user(NewUser {
            username: name.into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids: libs,
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login(name, "another long password", Default::default())
        .await
        .unwrap()
        .token;
    (common::with_token(app(e.state.clone()), &t), u.id)
}

async fn tmdb_json(s: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(s)
        .await;
}

fn movie_body() -> Value {
    json!({
        "id": 10, "title": "Heat", "release_date": "1995-12-15", "poster_path": "/heat.jpg",
        "vote_average": 7.9, "vote_count": 5000, "imdb_id": "tt0113277", "genres": [],
        "credits": {"cast": [
            {"id": 1158, "name": "Al Pacino", "character": "Vincent Hanna", "profile_path": "/pacino.jpg", "order": 0},
            {"id": 380, "name": "Robert De Niro", "character": "Neil McCauley", "profile_path": null, "order": 1}
        ], "crew": []}
    })
}

#[tokio::test]
async fn cast_carries_person_tmdb_id_in_library_and_discover() {
    let tmdb = MockServer::start().await;
    tmdb_json(&tmdb, "/movie/10", movie_body()).await;
    let e = env(&tmdb, None, None).await;
    // discover detail
    let (s, v) = get(&e.admin, "/api/v1/discover/movie/10").await;
    assert_eq!(s, 200, "{v}");
    let cast = v["cast"].as_array().unwrap();
    assert_eq!(cast.len(), 2);
    assert_eq!(cast[0]["name"], "Al Pacino");
    assert_eq!(cast[0]["character"], "Vincent Hanna");
    assert_eq!(cast[0]["person_tmdb_id"], 1158);
    assert!(
        cast[0]["photo_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/profile")
    );
    assert!(cast[1].get("photo_url").is_none());
    assert_eq!(v["card"]["status"], "none");
    assert!(v["card"].get("library_item_id").is_none());
    assert!(
        v.get("trailer_youtube_id").is_none(),
        "no videos, no trailer"
    );

    // library item: people stored by enrichment keep tmdb ids
    let db = &e.state.db;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id, poster) VALUES (5,1,'movie','Heat',10,'/p')").execute(db).await.unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (5,'/x.mkv',1,1)")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO people (id, tmdb_id, name, photo) VALUES (1,1158,'Al Pacino','/pacino.jpg'),(2,NULL,'Local Actor',NULL)").execute(db).await.unwrap();
    sqlx::query("INSERT INTO credits (item_id, person_id, role, character, ord) VALUES (5,1,'actor','Vincent',0),(5,2,'actor',NULL,1)").execute(db).await.unwrap();
    let (s, v) = get(&e.admin, "/api/v1/items/5").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["cast"][0]["person_tmdb_id"], 1158);
    assert!(
        v["cast"][1].get("person_tmdb_id").is_none(),
        "nfo-only people have no tmdb id"
    );
    // and the discover card now links to the library item
    let (_, v) = get(&e.admin, "/api/v1/discover/movie/10").await;
    assert_eq!(v["card"]["library_item_id"], 5);
    assert_eq!(v["card"]["status"], "available");
}

#[tokio::test]
async fn discover_detail_carries_the_tmdb_trailer() {
    let tmdb = MockServer::start().await;
    let mut body = movie_body();
    body["videos"] = json!({"results": [
        {"key": "clipAAAAAAA", "site": "YouTube", "type": "Clip", "official": true},
        {"key": "8g18jFHCLXk", "site": "YouTube", "type": "Trailer", "official": true}
    ]});
    tmdb_json(&tmdb, "/movie/10", body).await;
    tmdb_json(
        &tmdb,
        "/tv/20",
        json!({"id": 20, "name": "Show", "seasons": [{"season_number": 1}],
               "videos": {"results": [{"key": "tvTeaser001", "site": "YouTube", "type": "Teaser"}]}}),
    )
    .await;
    let e = env(&tmdb, None, None).await;
    let (s, v) = get(&e.admin, "/api/v1/discover/movie/10").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["trailer_youtube_id"], "8g18jFHCLXk");
    let (s, v) = get(&e.admin, "/api/v1/discover/tv/20").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["trailer_youtube_id"], "tvTeaser001");
}

#[tokio::test]
async fn person_page_dedupes_sorts_and_filters_credits() {
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/person/1158"))
        .and(query_param("append_to_response", "combined_credits,external_ids"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1158, "name": "Al Pacino", "biography": "Born in NYC.", "birthday": "1940-04-25",
            "deathday": null, "place_of_birth": "New York City, USA", "known_for_department": "Acting",
            "profile_path": "/pacino.jpg", "imdb_id": "nm0000199",
            "combined_credits": {
              "cast": [
                {"id": 10, "media_type": "movie", "title": "Heat", "release_date": "1995-12-15", "poster_path": "/heat.jpg", "character": "Vincent Hanna", "vote_average": 7.9, "vote_count": 7312, "popularity": 41.5},
                {"id": 20, "media_type": "movie", "title": "Newer", "release_date": "2019-01-01", "poster_path": "/n.jpg", "character": "Self"},
                {"id": 30, "media_type": "tv", "name": "Hunters", "first_air_date": "2020-02-21", "poster_path": "/h.jpg", "character": "Meyer"},
                {"id": 40, "media_type": "movie", "title": "No Poster But Dated", "release_date": "1980-01-01", "poster_path": null, "character": "X"},
                {"id": 50, "media_type": "movie", "title": "Bare Entry", "poster_path": null, "release_date": ""},
                {"id": 60, "media_type": "movie", "title": "Poster No Date", "poster_path": "/pnd.jpg"}
              ],
              "crew": [
                {"id": 10, "media_type": "movie", "title": "Heat", "release_date": "1995-12-15", "poster_path": "/heat.jpg", "job": "Executive Producer"},
                {"id": 70, "media_type": "movie", "title": "Directed", "release_date": "1996-05-01", "poster_path": "/d.jpg", "job": "Director"},
                {"id": 70, "media_type": "movie", "title": "Directed", "release_date": "1996-05-01", "poster_path": "/d.jpg", "job": "Writer"}
              ]
            }
        })))
        .expect(1) // second request is served from the 1 h cache
        .mount(&tmdb)
        .await;
    let e = env(&tmdb, None, None).await;
    let db = &e.state.db;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (5,1,'movie','Heat',10)").execute(db).await.unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (5,'/x.mkv',1,1)")
        .execute(db)
        .await
        .unwrap();

    let (s, v) = get(&e.admin, "/api/v1/people/1158").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["tmdb_id"].as_i64(), v["name"].as_str()),
        (Some(1158), Some("Al Pacino"))
    );
    assert_eq!(v["biography"], "Born in NYC.");
    assert_eq!(v["birthday"], "1940-04-25");
    assert!(v.get("deathday").is_none());
    assert_eq!(v["place_of_birth"], "New York City, USA");
    assert_eq!(v["known_for_department"], "Acting");
    assert_eq!(v["imdb_id"], "nm0000199");
    assert!(
        v["photo_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/profile")
    );
    let titles: Vec<&str> = v["credits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Hunters",
            "Newer",
            "Directed",
            "Heat",
            "No Poster But Dated",
            "Poster No Date"
        ],
        "date desc, undated last, 'Bare Entry' dropped, Heat once"
    );
    let heat = &v["credits"][3];
    assert_eq!(
        (heat["role"].as_str(), heat["character"].as_str()),
        (Some("cast"), Some("Vincent Hanna")),
        "cast preferred"
    );
    assert!(heat.get("job").is_none());
    // rating fields for client-side sorting (#141)
    assert_eq!(heat["vote_average"], 7.9);
    assert_eq!(heat["vote_count"], 7312);
    assert_eq!(heat["popularity"], 41.5);
    let newer = &v["credits"][1];
    assert!(
        newer.get("vote_count").is_none() && newer.get("popularity").is_none(),
        "absent upstream, absent here: {newer}"
    );
    assert_eq!(heat["status"], "available");
    assert_eq!(heat["library_item_id"], 5);
    assert_eq!(heat["media_type"], "movie");
    assert_eq!(heat["year"], 1995);
    assert!(
        heat["poster_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/poster")
    );
    let dir = &v["credits"][2];
    assert_eq!(
        (dir["role"].as_str(), dir["job"].as_str()),
        (Some("crew"), Some("Director, Writer"))
    );
    assert_eq!(v["credits"][0]["media_type"], "tv");
    assert_eq!(v["credits"][0]["status"], "none");

    assert_eq!(get(&e.admin, "/api/v1/people/1158").await.0, 200);
    assert_eq!(get(&e.admin, "/api/v1/people/999").await.0, 404);
}

async fn mock_reviews(tmdb: &MockServer) {
    tmdb_json(tmdb, "/movie/10", movie_body()).await;
    tmdb_json(
        tmdb,
        "/movie/10/reviews",
        json!({"results": [
            {"author": "old", "content": "Meh.", "created_at": "2020-01-01T00:00:00.000Z", "url": "https://tmdb/r/1",
             "author_details": {"avatar_path": "/https://secure.gravatar.com/avatar/x", "rating": null}},
            {"author": "new", "content": "Great film.", "created_at": "2024-05-05T00:00:00.000Z", "url": "https://tmdb/r/2",
             "author_details": {"avatar_path": "/av.jpg", "rating": 9.0}},
            {"author": "empty", "content": "  ", "created_at": "2025-01-01T00:00:00.000Z"}
        ]}),
    )
    .await;
}

#[tokio::test]
async fn reviews_merge_tmdb_omdb_and_trakt_with_caching() {
    let tmdb = MockServer::start().await;
    mock_reviews(&tmdb).await;
    let omdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("i", "tt0113277"))
        .and(query_param("apikey", "okey"))
        .and(query_param("tomatoes", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Response": "True", "imdbRating": "8.3", "imdbVotes": "700,123", "Metascore": "76",
            "Ratings": [{"Source": "Internet Movie Database", "Value": "8.3/10"}, {"Source": "Rotten Tomatoes", "Value": "87%"}],
            "tomatoURL": "https://www.rottentomatoes.com/m/heat_1995"
        })))
        .expect(1) // the 2nd call is served from the SQLite cache
        .mount(&omdb)
        .await;
    let trakt = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/tmdb/10"))
        .and(query_param("type", "movie"))
        .and(header("trakt-api-key", "tid"))
        .and(header("trakt-api-version", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"type": "movie", "score": null, "movie": {"title": "Heat", "ids": {"trakt": 1, "slug": "heat-1995", "tmdb": 10}}}
        ])))
        .mount(&trakt)
        .await;
    Mock::given(method("GET"))
        .and(path("/movies/heat-1995/comments/likes"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 99, "comment": "Trakt take.", "created_at": "2023-03-03T00:00:00.000Z", "user_rating": 8,
             "user": {"username": "bob", "name": "Bob B"}}
        ])))
        .mount(&trakt)
        .await;

    let e = env(&tmdb, Some(&omdb), Some(&trakt)).await;
    for _ in 0..2 {
        let (s, v) = get(&e.admin, "/api/v1/reviews/movie/10").await;
        assert_eq!(s, 200, "{v}");
        let r = v["ratings"].as_array().unwrap();
        let by = |src: &str| r.iter().find(|x| x["source"] == src).unwrap().clone();
        assert_eq!(
            r.iter()
                .map(|x| x["source"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["imdb", "rotten_tomatoes", "metacritic", "tmdb"]
        );
        assert_eq!(
            (
                by("imdb")["value"].as_str(),
                by("imdb")["score_100"].as_f64(),
                by("imdb")["votes"].as_i64()
            ),
            (Some("8.3/10"), Some(83.0), Some(700123))
        );
        assert_eq!(
            (
                by("rotten_tomatoes")["value"].as_str(),
                by("rotten_tomatoes")["score_100"].as_f64()
            ),
            (Some("87%"), Some(87.0))
        );
        assert_eq!(
            by("rotten_tomatoes")["url"],
            "https://www.rottentomatoes.com/m/heat_1995"
        );
        assert_eq!(by("metacritic")["value"], "76/100");
        assert_eq!(
            (
                by("tmdb")["value"].as_str(),
                by("tmdb")["votes"].as_i64(),
                by("tmdb")["label"].as_str()
            ),
            (Some("7.9/10"), Some(5000), Some("TMDB"))
        );
        assert_eq!(
            by("imdb")["url"],
            "https://www.imdb.com/title/tt0113277/reviews"
        );

        let rv = v["reviews"].as_array().unwrap();
        let authors: Vec<&str> = rv.iter().map(|x| x["author"].as_str().unwrap()).collect();
        assert_eq!(
            authors,
            ["new", "old", "Bob B"],
            "tmdb newest first (empty dropped), then trakt"
        );
        assert_eq!(
            (rv[0]["source"].as_str(), rv[0]["rating"].as_f64()),
            (Some("tmdb"), Some(9.0))
        );
        assert!(
            rv[0]["avatar_url"]
                .as_str()
                .unwrap()
                .contains("/images/tmdb/profile")
        );
        assert!(
            rv[1].get("avatar_url").is_none(),
            "external gravatar avatars are not proxied"
        );
        assert_eq!(
            (
                rv[2]["source"].as_str(),
                rv[2]["rating"].as_f64(),
                rv[2]["url"].as_str()
            ),
            (
                Some("trakt"),
                Some(8.0),
                Some("https://trakt.tv/comments/99")
            )
        );
        assert_eq!(rv[0]["created_at"], "2024-05-05T00:00:00.000Z");

        let links: std::collections::HashMap<&str, &str> = v["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (l["source"].as_str().unwrap(), l["url"].as_str().unwrap()))
            .collect();
        assert_eq!(links["letterboxd"], "https://letterboxd.com/tmdb/10");
        assert_eq!(links["tmdb"], "https://www.themoviedb.org/movie/10");
        assert_eq!(links["trakt"], "https://trakt.tv/movies/heat-1995");
        assert_eq!(
            links["rotten_tomatoes"],
            "https://www.rottentomatoes.com/m/heat_1995"
        );
        assert_eq!(
            links["imdb"],
            "https://www.imdb.com/title/tt0113277/reviews"
        );
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM omdb_cache WHERE imdb_id = 'tt0113277'")
        .fetch_one(&e.state.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn reviews_without_optional_keys_and_for_tv() {
    let tmdb = MockServer::start().await;
    tmdb_json(&tmdb, "/tv/20", json!({"id": 20, "name": "The Wire", "vote_average": 8.6, "vote_count": 2000, "external_ids": {"imdb_id": "tt0306414"}})).await;
    tmdb_json(&tmdb, "/tv/20/reviews", json!({"results": []})).await;
    let e = env(&tmdb, None, None).await;
    let (s, v) = get(&e.admin, "/api/v1/reviews/tv/20").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ratings"].as_array().unwrap().len(), 1, "TMDB only");
    assert_eq!(v["ratings"][0]["source"], "tmdb");
    assert_eq!(v["reviews"], json!([]));
    let links: Vec<&str> = v["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["source"].as_str().unwrap())
        .collect();
    assert_eq!(
        links,
        ["imdb", "rotten_tomatoes", "tmdb"],
        "no letterboxd for TV, no trakt without a client id"
    );
    let rt = v["links"][1]["url"].as_str().unwrap();
    assert_eq!(rt, "https://www.rottentomatoes.com/search?search=The+Wire");
    assert_eq!(get(&e.admin, "/api/v1/reviews/book/1").await.0, 404);
    assert_eq!(get(&e.admin, "/api/v1/reviews/movie/404").await.0, 404);
}

#[tokio::test]
async fn omdb_failure_is_silent() {
    let tmdb = MockServer::start().await;
    mock_reviews(&tmdb).await;
    let omdb = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&omdb)
        .await;
    let e = env(&tmdb, Some(&omdb), None).await;
    let (s, v) = get(&e.admin, "/api/v1/reviews/movie/10").await;
    assert_eq!(s, 200);
    assert_eq!(v["ratings"].as_array().unwrap().len(), 1);
}

fn radarr_queue() -> Value {
    json!({"page":1,"pageSize":500,"totalRecords":2,"records":[
        {"id": 11, "movieId": 1, "title": "Heat.1995.1080p", "size": 1000.0, "sizeleft": 250.0, "timeleft": "00:10:00",
         "status": "downloading", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok", "downloadId": "ABC",
         "movie": {"title": "Heat", "tmdbId": 10, "images": [{"coverType":"poster","remoteUrl":"https://image.tmdb.org/t/p/original/heat.jpg"}]}},
        {"id": 12, "movieId": 2, "title": "Other.2020", "size": 500.0, "sizeleft": 500.0,
         "status": "queued", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok",
         "movie": {"title": "Other", "tmdbId": 77, "images": [{"coverType":"poster","remoteUrl":"https://image.tmdb.org/t/p/original/other.jpg"}]}}
    ]})
}

fn sonarr_queue() -> Value {
    json!({"page":1,"pageSize":500,"totalRecords":2,"records":[
        {"id": 21, "seriesId": 5, "episodeId": 100, "seasonNumber": 1, "title": "Show.S01E02", "size": 200.0, "sizeleft": 0.0,
         "status": "completed", "trackedDownloadState": "importPending", "trackedDownloadStatus": "ok",
         "episode": {"seasonNumber": 1, "episodeNumber": 2, "title": "The Second"},
         "series": {"title": "The Show", "tvdbId": 555, "tmdbId": 0, "images": []}},
        {"id": 22, "seriesId": 6, "title": "Bad.S02E01", "size": 100.0, "sizeleft": 100.0,
         "status": "failed", "trackedDownloadState": "failed", "trackedDownloadStatus": "error", "errorMessage": "Download failed",
         "episode": {"seasonNumber": 2, "episodeNumber": 1},
         "series": {"title": "Bad Show", "tvdbId": 666, "images": []}}
    ]})
}

#[tokio::test]
async fn downloads_aggregate_radarr_and_sonarr_with_user_filtering() {
    let tmdb = MockServer::start().await;
    // TMDB poster fallback for the series whose queue has no artwork
    tmdb_json(
        &tmdb,
        "/tv/300",
        json!({"id": 300, "name": "The Show", "poster_path": "/show.jpg"}),
    )
    .await;
    let radarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .and(query_param("includeMovie", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(radarr_queue()))
        .expect(1) // two client calls inside the 2 s window → one Arr poll
        .mount(&radarr)
        .await;
    let sonarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .and(query_param("includeSeries", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sonarr_queue()))
        .expect(1)
        .mount(&sonarr)
        .await;

    let e = env(&tmdb, None, None).await;
    let db = &e.state.db;
    for (id, kind, name, url) in [
        (1, "radarr", "Radarr 4K", radarr.uri()),
        (2, "sonarr", "Sonarr", sonarr.uri()),
    ] {
        sqlx::query("INSERT INTO arr_servers (id, kind, name, url, api_key) VALUES (?,?,?,?,'k')")
            .bind(id)
            .bind(kind)
            .bind(name)
            .bind(url)
            .execute(db)
            .await
            .unwrap();
    }
    // library: Heat (movie 10) with a poster; The Show (tmdb 300, tvdb 555) as a show without one
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id, poster) VALUES (5,1,'movie','Heat',10,'/p')").execute(db).await.unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (5,'/x.mkv',1,1)")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id, tvdb_id) VALUES (6,1,'show','The Show',300,555)").execute(db).await.unwrap();

    let (alice, alice_id) = user_app(&e, "alice", vec![1]).await;
    let (bob, bob_id) = user_app(&e, "bob", vec![1]).await;
    // alice requested Heat (Radarr movie 1) and The Show S1 (Sonarr series 5); bob requested Bad Show
    for (user, mt, tmdb_id, arr, item, seasons) in [
        (alice_id, "movie", 10, 1, 1, None),
        (alice_id, "tv", 300, 2, 5, Some("[1]")),
        (bob_id, "tv", 400, 2, 6, Some("[2]")),
    ] {
        sqlx::query("INSERT INTO requests (user_id, media_type, tmdb_id, seasons_json, status, arr_kind, arr_id, arr_item_id, title) VALUES (?,?,?,?, 'downloading', ?, ?, ?, 'T')")
            .bind(user).bind(mt).bind(tmdb_id).bind(seasons)
            .bind(if mt == "movie" { "radarr" } else { "sonarr" }).bind(arr).bind(item)
            .execute(db).await.unwrap();
    }

    let (s, all) = get(&e.admin, "/api/v1/downloads").await;
    assert_eq!(s, 200, "{all}");
    let items = all.as_array().unwrap();
    assert_eq!(items.len(), 4);
    let by = |id: &str| items.iter().find(|i| i["id"] == id).unwrap().clone();

    let heat = by("1:11");
    assert_eq!(
        (
            heat["arr_server_id"].as_i64(),
            heat["arr_name"].as_str(),
            heat["media_type"].as_str()
        ),
        (Some(1), Some("Radarr 4K"), Some("movie"))
    );
    assert_eq!(
        (
            heat["title"].as_str(),
            heat["release_title"].as_str(),
            heat["tmdb_id"].as_i64()
        ),
        (Some("Heat"), Some("Heat.1995.1080p"), Some(10))
    );
    assert_eq!(
        (
            heat["state"].as_str(),
            heat["progress"].as_f64(),
            heat["eta_seconds"].as_i64()
        ),
        (Some("downloading"), Some(0.75), Some(600))
    );
    assert_eq!(
        (heat["size_bytes"].as_i64(), heat["sizeleft_bytes"].as_i64()),
        (Some(1000), Some(250))
    );
    assert_eq!(heat["library_item_id"], 5);
    assert!(heat["request_id"].is_i64());
    assert!(
        heat["poster_url"]
            .as_str()
            .unwrap()
            .contains("/images/poster/5"),
        "library poster preferred"
    );
    assert!(heat.get("speed_bps").is_none() && heat.get("error").is_none());

    let other = by("1:12");
    assert_eq!(
        (other["state"].as_str(), other["progress"].as_f64()),
        (Some("queued"), Some(0.0))
    );
    assert!(
        other.get("request_id").is_none(),
        "not started from Reel, still listed for admins"
    );
    assert!(other.get("library_item_id").is_none());
    assert!(
        other["poster_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/poster"),
        "TMDB poster from Radarr artwork"
    );

    let ep = by("2:21");
    assert_eq!(
        (
            ep["media_type"].as_str(),
            ep["title"].as_str(),
            ep["season"].as_i64(),
            ep["episode"].as_i64(),
            ep["episode_title"].as_str()
        ),
        (
            Some("tv"),
            Some("The Show"),
            Some(1),
            Some(2),
            Some("The Second")
        )
    );
    assert_eq!(
        (ep["state"].as_str(), ep["progress"].as_f64()),
        (Some("importing"), Some(1.0))
    );
    assert_eq!(
        (ep["tvdb_id"].as_i64(), ep["tmdb_id"].as_i64()),
        (Some(555), Some(300)),
        "tmdb mapped via request / library tvdb"
    );
    assert_eq!(ep["library_item_id"], 6);
    assert!(
        ep["poster_url"]
            .as_str()
            .unwrap()
            .contains("/images/tmdb/poster"),
        "show has no library poster → TMDB"
    );

    let bad = by("2:22");
    assert_eq!(
        (bad["state"].as_str(), bad["error"].as_str()),
        (Some("failed"), Some("Download failed"))
    );

    // alice only sees the downloads of her own requests (Heat; The Show S1 only: season 2 is bob's)
    let (_, mine) = get(&alice, "/api/v1/downloads").await;
    let ids: Vec<&str> = mine
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["1:11", "2:21"]);
    assert_eq!(mine[0]["library_item_id"], 5);
    // #15: once Heat sits in a library alice can't open, her download neither links to it nor
    // shows its library poster (the admin still gets both)
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (2,'Private','movies')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("UPDATE media_items SET library_id = 2 WHERE id = 5")
        .execute(db)
        .await
        .unwrap();
    let (_, mine) = get(&alice, "/api/v1/downloads").await;
    let heat = &mine[0];
    assert_eq!(heat["id"], "1:11");
    assert!(heat.get("library_item_id").is_none(), "{heat}");
    assert!(
        !heat["poster_url"]
            .as_str()
            .unwrap()
            .contains("/images/poster/5"),
        "{heat}"
    );
    let (_, all) = get(&e.admin, "/api/v1/downloads").await;
    let heat = all
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "1:11")
        .unwrap();
    assert_eq!(heat["library_item_id"], 5);
    let (_, bobs) = get(&bob, "/api/v1/downloads").await;
    let ids: Vec<&str> = bobs
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["2:22"]);
    let _ = (alice, bob);
}

#[tokio::test]
async fn downloads_empty_without_arr_servers_and_skips_dead_ones() {
    let tmdb = MockServer::start().await;
    let e = env(&tmdb, None, None).await;
    let (s, v) = get(&e.admin, "/api/v1/downloads").await;
    assert_eq!((s, v), (StatusCode::OK, json!([])));
    // an unreachable server does not fail the call
    sqlx::query("INSERT INTO arr_servers (id, kind, name, url, api_key) VALUES (1,'radarr','Dead','http://127.0.0.1:9','k')")
        .execute(&e.state.db).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let (s, v) = get(&e.admin, "/api/v1/downloads").await;
    assert_eq!((s, v), (StatusCode::OK, json!([])));
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let tmdb = MockServer::start().await;
    let e = env(&tmdb, None, None).await;
    let raw = app(e.state.clone());
    for p in [
        "/api/v1/downloads",
        "/api/v1/people/1",
        "/api/v1/reviews/movie/1",
    ] {
        assert_eq!(get(&raw, p).await.0, 401, "{p}");
    }
}

#[tokio::test]
async fn downloads_are_pushed_over_websocket_while_active() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let tmdb = MockServer::start().await;
    let radarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(radarr_queue()))
        .up_to_n_times(1)
        .mount(&radarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"page":1,"pageSize":500,"totalRecords":0,"records":[]})),
        )
        .mount(&radarr)
        .await;
    let e = env(&tmdb, None, None).await;
    sqlx::query(
        "INSERT INTO arr_servers (id, kind, name, url, api_key) VALUES (1,'radarr','R',?,'k')",
    )
    .bind(radarr.uri())
    .execute(&e.state.db)
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = e.admin.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events"))
        .await
        .unwrap();

    let mut counts = vec![];
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(end, ws.next()).await {
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] != "downloads" {
            continue;
        }
        let items = v["items"].as_array().unwrap();
        if let Some(first) = items.first() {
            assert_eq!(first["id"], "1:11");
        }
        counts.push(items.len());
        if items.is_empty() {
            break;
        }
    }
    assert_eq!(
        counts,
        [2, 0],
        "pushed once when active, once when the queue drains (no repeats)"
    );
}

#[tokio::test]
async fn downloads_explain_stuck_items_via_arr_messages_and_torrent_client_state() {
    let radarr = MockServer::start().await;
    let rec = |id: i64, dl: &str, size: f64, left: f64, msgs: Value| {
        json!({"id": id, "movieId": id, "title": format!("Rel.{id}"), "size": size, "sizeleft": left,
            "status": "queued", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok",
            "downloadId": dl, "errorMessage": null, "statusMessages": msgs,
            "movie": {"title": format!("M{id}"), "tmdbId": 100 + id, "images": []}})
    };
    let queue = json!({"page":1,"pageSize":500,"totalRecords":5,"records":[
        // Arr message wins over the client text
        rec(1, "H1", 0.0, 0.0, json!([{"title":"Rel.1","messages":["qBittorrent is downloading metadata"]}])),
        // metadata, nothing from the Arr: client state explains it, and it becomes stalled after the window
        rec(2, "H2", 0.0, 0.0, json!([])),
        rec(3, "H3", 1000.0, 1000.0, json!([])),
        rec(4, "H4", 1000.0, 400.0, json!([])),
        rec(5, "H5", 1000.0, 1000.0, json!([])),
    ]});
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue))
        .mount(&radarr)
        .await;
    let qb = MockServer::start().await;
    let t = |h: &str, state: &str, progress: f64| json!({"hash": h, "progress": progress, "dlspeed": 0, "eta": 8640000, "num_seeds": 0, "num_leechs": 0, "state": state, "size": 1000});
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            t("H1", "metaDL", 0.0),
            t("H2", "metaDL", 0.0),
            t("H3", "queuedDL", 0.0),
            t("H4", "stalledDL", 0.4),
            t("H5", "pausedDL", 0.0)
        ])))
        .mount(&qb)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let opts = reel_requests::Options {
        tracker: reel_requests::TrackerConfig {
            stall_after: std::time::Duration::from_secs(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let svc =
        reel_requests::RequestsService::with_options(db.clone(), None, state.events.clone(), opts);
    let state = state.with_requests(svc);
    let admin = common::authed(&state, app(state.clone())).await;
    sqlx::query(
        "INSERT INTO arr_servers (id, kind, name, url, api_key) VALUES (1,'radarr','R',?,'k')",
    )
    .bind(radarr.uri())
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO download_clients (kind, url) VALUES ('qbittorrent', ?)")
        .bind(qb.uri())
        .execute(&db)
        .await
        .unwrap();

    let snap = |v: &Value| -> std::collections::HashMap<String, Value> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|i| (i["id"].as_str().unwrap().to_string(), i.clone()))
            .collect()
    };
    let (_, v) = get(&admin, "/api/v1/downloads").await;
    let m = snap(&v);
    assert_eq!(
        m["1:1"]["status_text"],
        "qBittorrent is downloading metadata"
    );
    assert_eq!(m["1:1"]["client_state"], "metaDL");
    assert_eq!(
        m["1:1"]["state"], "queued",
        "fresh metadata fetch is not yet stalled"
    );
    assert_eq!(
        (
            m["1:2"]["status_text"].as_str(),
            m["1:2"]["client_state"].as_str()
        ),
        (Some("Fetching metadata"), Some("metaDL"))
    );
    assert_eq!(m["1:2"]["state"], "queued");
    assert_eq!(
        (m["1:3"]["status_text"].as_str(), m["1:3"]["state"].as_str()),
        (Some("Queued in client"), Some("queued"))
    );
    assert_eq!(
        (
            m["1:4"]["status_text"].as_str(),
            m["1:4"]["client_state"].as_str()
        ),
        (Some("Stalled — no peers"), Some("stalledDL"))
    );
    assert_eq!(m["1:4"]["state"], "stalled", "client says stalled");
    assert_eq!(
        (m["1:5"]["status_text"].as_str(), m["1:5"]["state"].as_str()),
        (Some("Paused"), Some("stalled"))
    );

    // after the stall window, 0-byte / metadata items that never moved become stalled
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let (_, v) = get(&admin, "/api/v1/downloads").await;
    let m = snap(&v);
    for id in ["1:1", "1:2"] {
        assert_eq!(m[id]["state"], "stalled", "{id}");
    }
    assert_eq!(
        m["1:3"]["state"], "queued",
        "a sized item waiting in the client is just queued"
    );
    assert_eq!(m["1:2"]["status_text"], "Fetching metadata", "text kept");
}

/// #15: a card links to a library item only when the viewer may open it. The discovery service
/// picks the first copy server-wide; a copy in a library the viewer can't see must not leak (its
/// id says the title is there), and a copy in a library they can see is linked instead.
#[tokio::test]
async fn discover_cards_link_only_library_items_the_viewer_can_open() {
    let tmdb = MockServer::start().await;
    tmdb_json(&tmdb, "/movie/10", movie_body()).await;
    tmdb_json(
        &tmdb,
        "/trending/all/week",
        json!({"page": 1, "total_pages": 1, "total_results": 1, "results": [
            {"id": 10, "media_type": "movie", "title": "Heat", "release_date": "1995-12-15", "poster_path": "/heat.jpg"}
        ]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/person/1158"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1158, "name": "Al Pacino",
            "combined_credits": {"cast": [
                {"id": 10, "media_type": "movie", "title": "Heat", "release_date": "1995-12-15", "poster_path": "/heat.jpg", "character": "Vincent Hanna"}
            ], "crew": []}
        })))
        .mount(&tmdb)
        .await;
    let e = env(&tmdb, None, None).await;
    let db = &e.state.db;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'Movies','movies'),(2,'Private 4K','movies')")
        .execute(db)
        .await
        .unwrap();
    // Heat exists only in the library alice can't see
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (5,2,'movie','Heat',10)")
        .execute(db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_files (item_id, path, size, mtime) VALUES (5,'/4k/heat.mkv',1,1)",
    )
    .execute(db)
    .await
    .unwrap();
    let (alice, _) = user_app(&e, "alice", vec![1]).await;

    let detail =
        |app: Router| async move { get(&app, "/api/v1/discover/movie/10").await.1["card"].clone() };
    let trending = |app: Router| async move {
        get(&app, "/api/v1/discover/trending").await.1["results"][0].clone()
    };
    let person =
        |app: Router| async move { get(&app, "/api/v1/people/1158").await.1["credits"][0].clone() };

    for (what, card) in [
        ("detail", detail(alice.clone()).await),
        ("trending", trending(alice.clone()).await),
        ("person", person(alice.clone()).await),
    ] {
        assert_eq!(card["tmdb_id"], 10, "{what}: {card}");
        assert!(
            card.get("library_item_id").is_none(),
            "{what} leaks the hidden item: {card}"
        );
    }
    // the admin sees every library
    assert_eq!(detail(e.admin.clone()).await["library_item_id"], 5);
    assert_eq!(trending(e.admin.clone()).await["library_item_id"], 5);
    assert_eq!(person(e.admin.clone()).await["library_item_id"], 5);

    // a second copy in alice's library: she gets that one, not the lower hidden id
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (7,1,'movie','Heat',10)")
        .execute(db)
        .await
        .unwrap();
    // without a file a movie isn't playable: still no link
    assert!(detail(alice.clone()).await.get("library_item_id").is_none());
    sqlx::query(
        "INSERT INTO media_files (item_id, path, size, mtime) VALUES (7,'/m/heat.mkv',1,1)",
    )
    .execute(db)
    .await
    .unwrap();
    assert_eq!(detail(alice.clone()).await["library_item_id"], 7);
    assert_eq!(trending(alice.clone()).await["library_item_id"], 7);
    assert_eq!(person(alice.clone()).await["library_item_id"], 7);
    assert_eq!(detail(e.admin.clone()).await["library_item_id"], 5);
}

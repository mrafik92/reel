#![allow(clippy::type_complexity)]
use reel_metadata::{TmdbClient, TmdbEnricher};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

async fn pool() -> sqlx::SqlitePool {
    let p = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1, 'L', 'movies')")
        .execute(&p)
        .await
        .unwrap();
    p
}

async fn mock_movie(s: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .and(query_param("query", "Dune"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"id":438631,"title":"Dune","release_date":"2021-09-15","popularity":9.0}]
        })))
        .mount(s)
        .await;
    let cast: Vec<_> = (0..30)
        .map(|i| json!({"id":1000+i,"name":format!("Actor {i}"),"character":format!("C{i}"),"order":i}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/movie/438631"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":438631,"title":"Dune","release_date":"2021-09-15","overview":"Remote plot",
            "vote_average":7.8,"runtime":155,"poster_path":"/p.jpg","backdrop_path":"/b.jpg",
            "imdb_id":"tt1160419","genres":[{"id":1,"name":"Sci-Fi"},{"id":2,"name":"Adventure"}],
            "belongs_to_collection":{"id":726871,"name":"Dune Collection"},
            "credits":{"cast":cast,"crew":[{"id":5,"name":"Denis V","job":"Director"}]},
            "release_dates":{"results":[{"iso_3166_1":"US","release_dates":[{"certification":"PG-13"}]}]},
            "external_ids":{"imdb_id":"tt1160419","tvdb_id":null},
            "videos":{"results":[
                {"key":"n9xhJrPXop4","site":"YouTube","type":"Trailer","official":true,"published_at":"2020-09-09"},
                {"key":"8g18jFHCLXk","site":"YouTube","type":"Trailer","official":true,"published_at":"2021-07-22"}]}
        })))
        .mount(s)
        .await;
}

#[tokio::test]
async fn enriches_movie_with_lock_and_local_art() {
    let s = MockServer::start().await;
    mock_movie(&s).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Dune (2021).mkv");
    std::fs::write(&file, b"x").unwrap();
    std::fs::write(dir.path().join("poster.jpg"), b"img").unwrap();
    std::fs::write(
        dir.path().join("Dune (2021).nfo"),
        "<movie><title>Dune Local</title><uniqueid type=\"tmdb\">438631</uniqueid></movie>",
    )
    .unwrap();

    let p = pool().await;
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, year, overview, locked_fields_json) VALUES (1,1,'movie','Dune',2021,'My plot','[\"overview\"]')")
        .execute(&p).await.unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (1, ?, 1, 1)")
        .bind(file.to_string_lossy().as_ref())
        .execute(&p)
        .await
        .unwrap();

    let e = TmdbEnricher::new(TmdbClient::with_base_url("k", s.uri()));
    e.enrich(&p, 1).await.unwrap();

    let r: (String, Option<String>, i64, Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<f64>) =
        sqlx::query_as("SELECT title, sort_title, year, overview, content_rating, imdb_id, tmdb_id, poster, backdrop, runtime_s, rating FROM media_items WHERE id=1")
            .fetch_one(&p).await.unwrap();
    assert_eq!(r.0, "Dune Local"); // nfo wins over TMDB
    assert_eq!(r.1.as_deref(), Some("dune local"));
    assert_eq!(r.2, 2021);
    assert_eq!(r.3.as_deref(), Some("My plot")); // locked
    assert_eq!(r.4.as_deref(), Some("PG-13"));
    assert_eq!(r.5.as_deref(), Some("tt1160419"));
    assert_eq!(r.6, Some(438631));
    assert!(r.7.unwrap().starts_with("file://")); // local poster wins
    assert_eq!(r.8.as_deref(), Some("/b.jpg"));
    assert_eq!(r.9, Some(155 * 60));
    assert_eq!(r.10, Some(7.8));
    let (trailer, checked): (Option<String>, bool) = sqlx::query_as(
        "SELECT trailer_youtube_id, trailer_checked_at IS NOT NULL FROM media_items WHERE id=1",
    )
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!((trailer.as_deref(), checked), (Some("n9xhJrPXop4"), true));

    let (g,): (i64,) = sqlx::query_as("SELECT count(*) FROM item_genres WHERE item_id=1")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(g, 2);
    let (a,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM credits WHERE item_id=1 AND role='actor'")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(a, 20);
    let (d,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM credits WHERE item_id=1 AND role='director'")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(d, 1);
    let (c,): (i64,) = sqlx::query_as("SELECT count(*) FROM collection_items")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(c, 1);

    // idempotent
    e.enrich(&p, 1).await.unwrap();
    let (a,): (i64,) = sqlx::query_as("SELECT count(*) FROM credits WHERE item_id=1")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(a, 21);
}

#[tokio::test]
async fn unmatched_movie_left_alone() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[
            {"id":1,"title":"Totally Unrelated","release_date":"1990-01-01"}]})))
        .mount(&s)
        .await;
    let p = pool().await;
    sqlx::query(
        "INSERT INTO media_items (id, library_id, kind, title) VALUES (1,1,'movie','Zzyzx Qwerty')",
    )
    .execute(&p)
    .await
    .unwrap();
    TmdbEnricher::new(TmdbClient::with_base_url("k", s.uri()))
        .enrich(&p, 1)
        .await
        .unwrap();
    let (t, id): (String, Option<i64>) =
        sqlx::query_as("SELECT title, tmdb_id FROM media_items WHERE id=1")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(t, "Zzyzx Qwerty");
    assert_eq!(id, None);
}

#[tokio::test]
async fn enriches_show_seasons_and_episodes() {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/tv/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":42,"name":"Show","first_air_date":"2010-01-01","overview":"o","episode_run_time":[45],
            "genres":[{"id":1,"name":"Drama"}],"seasons":[{"season_number":1}],
            "external_ids":{"imdb_id":"tt9","tvdb_id":77},
            "content_ratings":{"results":[{"iso_3166_1":"US","rating":"TV-MA"}]},
            "credits":{"cast":[{"id":1,"name":"A","character":"X","order":0}],"crew":[]},
            "videos":{"results":[{"key":"tvTrailer01","site":"YouTube","type":"Teaser","official":true}]}
        }))).mount(&s).await;
    Mock::given(method("GET")).and(path("/tv/42/season/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "season_number":1,"name":"Season 1","overview":"so","poster_path":"/sp.jpg",
            "episodes":[
              {"episode_number":1,"name":"Pilot","overview":"eo","air_date":"2010-01-01","still_path":"/s1.jpg"},
              {"episode_number":2,"name":"Two","still_path":"/s2.jpg"}]
        }))).mount(&s).await;
    let p = pool().await;
    for q in [
        "INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (1,1,'show','show',42)",
        "INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no) VALUES (2,1,'season',1,'S1',1)",
        "INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no, episode_no, locked_fields_json) VALUES (3,1,'episode',2,'e1',1,1,'[\"title\"]')",
        "INSERT INTO media_items (id, library_id, kind, parent_id, title, season_no, episode_no) VALUES (4,1,'episode',2,'e2',1,2)",
    ] {
        sqlx::query(q).execute(&p).await.unwrap();
    }
    // enriching an episode routes to the show
    TmdbEnricher::new(TmdbClient::with_base_url("k", s.uri()))
        .enrich(&p, 3)
        .await
        .unwrap();
    let (t, cr, tv): (String, Option<String>, Option<i64>) =
        sqlx::query_as("SELECT title, content_rating, tvdb_id FROM media_items WHERE id=1")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(
        (t.as_str(), cr.as_deref(), tv),
        ("Show", Some("TV-MA"), Some(77))
    );
    let trailer: Option<String> =
        sqlx::query_scalar("SELECT trailer_youtube_id FROM media_items WHERE id=1")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(trailer.as_deref(), Some("tvTrailer01"));
    let (t, ov, still): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT title, overview, poster FROM media_items WHERE id=3")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(t, "e1"); // locked
    assert_eq!(ov.as_deref(), Some("eo"));
    assert_eq!(still.as_deref(), Some("/s1.jpg"));
    let (t,): (String,) = sqlx::query_as("SELECT title FROM media_items WHERE id=4")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(t, "Two");
    let (t,): (String,) = sqlx::query_as("SELECT title FROM media_items WHERE id=2")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(t, "Season 1");
}

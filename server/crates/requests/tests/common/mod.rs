#![allow(dead_code)]
use std::time::Duration;

use reel_core::{RequestState, ServerEvent};
use reel_metadata::TmdbClient;
use reel_requests::{Options, RequestsService, TrackerConfig, notify::RetryPolicy};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

pub async fn pool() -> SqlitePool {
    let p = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role) VALUES (2, 'bob', '!', 'user')",
    )
    .execute(&p)
    .await
    .unwrap();
    p
}

pub fn fast_cfg() -> Options {
    Options {
        tracker: TrackerConfig {
            poll_interval: Duration::from_millis(25),
            stall_after: Duration::from_secs(600),
            search_grace: Duration::from_secs(600),
            emit_throttle: Duration::ZERO,
            idle_recheck: Duration::from_secs(30),
            queue_page_size: 100,
        },
        notify_retry: RetryPolicy {
            attempts: 1,
            base_delay: Duration::from_millis(1),
        },
    }
}

pub fn service(
    pool: &SqlitePool,
    tmdb: Option<&MockServer>,
    opts: Options,
) -> (RequestsService, broadcast::Receiver<ServerEvent>) {
    let (tx, rx) = broadcast::channel(256);
    let t = tmdb.map(|m| TmdbClient::with_base_url("key", m.uri()));
    (RequestsService::with_options(pool.clone(), t, tx, opts), rx)
}

pub async fn add_arr(pool: &SqlitePool, kind: &str, url: &str) -> i64 {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO arr_servers (kind, name, url, api_key, default_profile, default_root)
         VALUES (?, ?, ?, 'secret', 4, '/data/root') RETURNING id",
    )
    .bind(kind)
    .bind(kind)
    .bind(url)
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

pub async fn mock_tmdb_movie(s: &MockServer, id: i64, title: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/movie/{id}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": id, "title": title, "poster_path": "/p.jpg"})),
        )
        .mount(s)
        .await;
}

pub async fn mock_radarr_add(s: &MockServer, tmdb: i64, movie_id: i64) {
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .and(query_param("tmdbId", tmdb.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie/lookup/tmdb"))
        .and(query_param("tmdbId", tmdb.to_string()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"title":"The Matrix","tmdbId":tmdb,"year":1999,"images":[]})),
        )
        .mount(s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"id":movie_id,"title":"The Matrix","tmdbId":tmdb,"monitored":true,"hasFile":false}),
        ))
        .mount(s)
        .await;
}

pub fn queue_body(records: Vec<Value>) -> Value {
    json!({"page":1,"pageSize":100,"totalRecords":records.len(),"records":records})
}

pub fn q_item(id: i64, movie_id: i64, size: f64, left: f64, status: &str, tds: &str) -> Value {
    json!({
        "id": id, "movieId": movie_id, "title": "Matrix.1999.1080p", "size": size, "sizeleft": left,
        "timeleft": "00:09:00", "status": status, "trackedDownloadState": tds,
        "trackedDownloadStatus": "ok", "downloadId": "ABC123"
    })
}

/// Collect `(state, progress%)` for one request until `until` state is seen (or timeout).
pub async fn collect_until(
    rx: &mut broadcast::Receiver<ServerEvent>,
    request_id: i64,
    until: RequestState,
) -> Vec<(RequestState, u32)> {
    let mut out = vec![];
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Ok(ev)) = tokio::time::timeout_at(end, rx.recv()).await {
        if let ServerEvent::RequestProgress {
            request_id: id,
            state,
            progress,
            ..
        } = ev
        {
            if id != request_id {
                continue;
            }
            out.push((state, (progress * 100.0).round() as u32));
            if state == until {
                break;
            }
        }
    }
    out
}

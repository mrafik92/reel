mod common;

use std::time::Duration;

use common::*;
use reel_core::RequestState::*;
use reel_requests::MediaType;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

async fn mount_queue_once(s: &MockServer, records: Vec<Value>) {
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(records)))
        .up_to_n_times(1)
        .mount(s)
        .await;
}

#[tokio::test]
async fn tracker_end_to_end_radarr() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    Mock::given(method("GET"))
        .and(path("/api/v3/command"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&radarr)
        .await;
    // Scripted queue: empty -> queued 0% -> 10% -> 60% -> importPending -> gone.
    mount_queue_once(&radarr, vec![]).await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 1000.0, "queued", "downloading")],
    )
    .await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 900.0, "downloading", "downloading")],
    )
    .await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 400.0, "downloading", "downloading")],
    )
    .await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 0.0, "completed", "importPending")],
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(vec![])))
        .mount(&radarr)
        .await;

    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, mut rx) = service(&pool, None, fast_cfg());
    let tracker = svc.spawn_tracker();

    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    let evs = collect_until(&mut rx, r.id, Importing).await;
    assert_eq!(
        evs,
        vec![
            (Requested, 0),
            (Approved, 0),
            (Searching, 0),
            (Queued, 0),
            (Downloading, 10),
            (Downloading, 60),
            (Importing, 100),
        ]
    );
    // Persisted state transitions + download row.
    let (status,): (String,) = sqlx::query_as("SELECT status FROM requests WHERE id = ?")
        .bind(r.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "importing");
    let (dl_state, title, size): (String, String, i64) = sqlx::query_as(
        "SELECT state, release_title, size FROM request_downloads WHERE request_id = ? AND download_id = 'ABC123'",
    )
    .bind(r.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (dl_state.as_str(), title.as_str(), size),
        ("importing", "Matrix.1999.1080p", 1000)
    );

    // Item left the queue: stays importing (no spurious transition) …
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(svc.get(r.id).await.unwrap().status, Importing);

    // … webhook Download confirms and hands back the path for a targeted rescan …
    let dl = json!({"eventType":"Download","movie":{"id":42,"tmdbId":603,"folderPath":"/m/Matrix"},"movieFile":{"relativePath":"Matrix.mkv"}});
    let o = svc
        .handle_webhook(dl.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert_eq!(o.imported_paths, vec!["/m/Matrix/Matrix.mkv"]);
    assert!(o.request_ids.is_empty()); // already importing: no change

    // … then the scanner glue flips it to available.
    svc.mark_available_by_tmdb(603).await.unwrap();
    let evs = collect_until(&mut rx, r.id, Available).await;
    assert_eq!(evs, vec![(Available, 100)]);
    assert_eq!(svc.get(r.id).await.unwrap().status, Available);
    tracker.abort();
}

#[tokio::test]
async fn tracker_marks_available_when_library_has_file() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 0.0, "completed", "importPending")],
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(vec![])))
        .mount(&radarr)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, mut rx) = service(&pool, None, fast_cfg());
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    collect_until(&mut rx, r.id, Importing).await;
    // scanner ingests the file
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (1,1,'movie','Matrix',603)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (1,'/m.mkv',1,1)")
        .execute(&pool)
        .await
        .unwrap();
    let evs = collect_until(&mut rx, r.id, Available).await;
    assert_eq!(evs.last(), Some(&(Available, 100)));
    tracker.abort();
}

#[tokio::test]
async fn tracker_stalled_and_recovery() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    // Frozen at 50% for a while, then moves.
    let frozen = q_item(1, 42, 1000.0, 500.0, "downloading", "downloading");
    for _ in 0..6 {
        mount_queue_once(&radarr, vec![frozen.clone()]).await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(queue_body(vec![q_item(
                1,
                42,
                1000.0,
                300.0,
                "downloading",
                "downloading",
            )])),
        )
        .mount(&radarr)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let mut cfg = fast_cfg();
    cfg.tracker.stall_after = Duration::from_millis(60);
    let (svc, mut rx) = service(&pool, None, cfg);
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    let evs = collect_until(&mut rx, r.id, Stalled).await;
    assert!(evs.contains(&(Downloading, 50)));
    assert_eq!(evs.last(), Some(&(Stalled, 50)));
    assert_eq!(svc.get(r.id).await.unwrap().status, Stalled);
    // bytes move again -> back to downloading
    let evs = collect_until(&mut rx, r.id, Downloading).await;
    assert_eq!(evs.last(), Some(&(Downloading, 70)));
    tracker.abort();
}

#[tokio::test]
async fn tracker_warning_then_failed() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    let mut warn = q_item(1, 42, 1000.0, 500.0, "downloading", "downloading");
    warn["trackedDownloadStatus"] = json!("warning");
    mount_queue_once(&radarr, vec![warn]).await;
    let mut failed = q_item(1, 42, 1000.0, 500.0, "failed", "failed");
    failed["errorMessage"] = json!("disk full");
    mount_queue_once(&radarr, vec![failed]).await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(vec![])))
        .mount(&radarr)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, mut rx) = service(&pool, None, fast_cfg());
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    let evs = collect_until(&mut rx, r.id, Failed).await;
    let states: Vec<_> = evs.iter().map(|e| e.0).collect();
    assert_eq!(
        states,
        vec![Requested, Approved, Searching, Stalled, Failed]
    );
    assert_eq!(svc.get(r.id).await.unwrap().status, Failed);
    tracker.abort();
}

#[tokio::test]
async fn tracker_vanished_item_without_file_returns_to_searching_then_not_found() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    mount_queue_once(
        &radarr,
        vec![q_item(1, 42, 1000.0, 800.0, "downloading", "downloading")],
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(vec![])))
        .mount(&radarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie/42"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":42,"tmdbId":603,"hasFile":false})),
        )
        .mount(&radarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/command"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id":3,"name":"MoviesSearch","status":"completed","body":{"movieIds":[42]}},
            {"id":2,"name":"MoviesSearch","status":"completed","body":{"movieIds":[7]}}
        ])))
        .mount(&radarr)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let mut cfg = fast_cfg();
    cfg.tracker.search_grace = Duration::from_millis(50);
    let (svc, mut rx) = service(&pool, None, cfg);
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    let evs = collect_until(&mut rx, r.id, NotFound).await;
    let states: Vec<_> = evs.iter().map(|e| e.0).collect();
    assert_eq!(
        states,
        vec![
            Requested,
            Approved,
            Searching,
            Downloading,
            Searching,
            NotFound
        ]
    );
    // not_found is not polled any more; retry restarts the search.
    Mock::given(method("POST"))
        .and(path("/api/v3/command"))
        .and(body_partial_json(json!({"name":"MoviesSearch"})))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"id":9,"name":"MoviesSearch","status":"queued"})),
        )
        .expect(1)
        .mount(&radarr)
        .await;
    let r2 = svc.retry(r.id).await.unwrap();
    assert_eq!(r2.status, Searching);
    tracker.abort();
}

#[tokio::test]
async fn sonarr_multi_episode_aggregation_end_to_end() {
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/1396"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":1396,"name":"Breaking Bad","poster_path":"/b.jpg",
            "external_ids":{"tvdb_id":81189,"imdb_id":"tt0903747"}
        })))
        .mount(&tmdb)
        .await;
    let sonarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/series"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&sonarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/series/lookup"))
        .and(wiremock::matchers::query_param("term", "tvdb:81189"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "title":"Breaking Bad","tvdbId":81189,
            "seasons":[{"seasonNumber":0,"monitored":false},{"seasonNumber":1,"monitored":false},{"seasonNumber":2,"monitored":false}]
        }])))
        .mount(&sonarr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/series"))
        .and(body_partial_json(json!({
            "tvdbId":81189,"qualityProfileId":4,"rootFolderPath":"/data/root","monitored":true,
            "addOptions":{"searchForMissingEpisodes":true},
            "seasons":[{"seasonNumber":0,"monitored":false},{"seasonNumber":1,"monitored":true},{"seasonNumber":2,"monitored":false}]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":9,"title":"Breaking Bad","tvdbId":81189})))
        .expect(1)
        .mount(&sonarr)
        .await;

    let ep = |id: i64,
              season: i64,
              num: i64,
              size: f64,
              left: f64,
              status: &str,
              tds: &str,
              time: &str| {
        json!({
            "id": id, "seriesId": 9, "episodeId": id, "seasonNumber": season, "title": format!("rel{id}"),
            "size": size, "sizeleft": left, "timeleft": time, "status": status,
            "trackedDownloadState": tds, "trackedDownloadStatus": "ok", "downloadId": format!("H{id}"),
            "episode": {"seasonNumber": season, "episodeNumber": num, "title": "x"}
        })
    };
    let other_series = json!({"id": 99, "seriesId": 5, "seasonNumber": 1, "size": 100.0, "sizeleft": 0.0, "status":"downloading"});
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queue_body(vec![
            ep(
                1,
                1,
                1,
                1000.0,
                0.0,
                "completed",
                "importPending",
                "00:00:00",
            ),
            ep(
                2,
                1,
                2,
                3000.0,
                3000.0,
                "downloading",
                "downloading",
                "00:30:00",
            ),
            ep(
                3,
                2,
                1,
                500.0,
                0.0,
                "downloading",
                "downloading",
                "00:00:01",
            ), // season 2: not requested
            other_series,
        ])))
        .mount(&sonarr)
        .await;

    let pool = pool().await;
    add_arr(&pool, "sonarr", &sonarr.uri()).await;
    let (svc, mut rx) = service(&pool, Some(&tmdb), fast_cfg());
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Tv, 1396, Some(vec![1]))
        .await
        .unwrap();
    assert_eq!(r.arr_item_id, Some(9));
    assert_eq!(r.title.as_deref(), Some("Breaking Bad"));
    assert_eq!(r.seasons, Some(vec![1]));
    let evs = collect_until(&mut rx, r.id, Downloading).await;
    assert_eq!(evs.last(), Some(&(Downloading, 25))); // (4000-3000)/4000

    let snap = svc.active_snapshot().await.unwrap();
    let p = snap.iter().find(|p| p.request_id == r.id).unwrap();
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.eta_seconds, Some(1800));
    assert_eq!(p.size_bytes, Some(4000));
    assert_eq!(p.items[0].episode, Some(1));
    assert_eq!(p.items[0].state, Importing);
    assert_eq!(p.items[1].state, Downloading);
    // per-item rows persisted on the state transition
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM request_downloads WHERE request_id = ?")
            .bind(r.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 2);
    tracker.abort();
}

#[tokio::test]
async fn torrent_client_enriches_progress() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    Mock::given(method("GET"))
        .and(path("/api/v3/queue"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(queue_body(vec![q_item(
                1,
                42,
                1000.0,
                900.0,
                "downloading",
                "downloading",
            )])),
        )
        .mount(&radarr)
        .await;
    let qb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .and(wiremock::matchers::query_param("hashes", "abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"hash":"ABC123","progress":0.123,"dlspeed":2500000,"eta":77,"num_seeds":8,"num_leechs":2,"state":"downloading","size":1000}
        ])))
        .mount(&qb)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    sqlx::query("INSERT INTO download_clients (kind, url) VALUES ('qbittorrent', ?)")
        .bind(qb.uri())
        .execute(&pool)
        .await
        .unwrap();
    let (svc, mut rx) = service(&pool, None, fast_cfg());
    let tracker = svc.spawn_tracker();
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(end, rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let reel_core::ServerEvent::RequestProgress {
            state: Downloading,
            progress,
            eta_seconds,
            speed_bps,
            ..
        } = ev
        {
            assert!((progress - 0.123).abs() < 1e-4);
            assert_eq!(eta_seconds, Some(77));
            assert_eq!(speed_bps, Some(2_500_000));
            break;
        }
    }
    let snap = svc.active_snapshot().await.unwrap();
    let p = snap.iter().find(|p| p.request_id == r.id).unwrap();
    assert_eq!((p.seeds, p.peers), (Some(8), Some(2)));
    tracker.abort();
}

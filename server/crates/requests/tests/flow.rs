mod common;

use common::*;
use reel_core::RequestState::*;
use reel_requests::{Availability, Error, ListFilter, MediaType};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, header, method, path, query_param},
};

#[tokio::test]
async fn radarr_add_flow_for_admin() {
    let radarr = MockServer::start().await;
    let tmdb = MockServer::start().await;
    mock_tmdb_movie(&tmdb, 603, "The Matrix").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .and(query_param("tmdbId", "603"))
        .and(header("x-api-key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&radarr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie/lookup/tmdb"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"title":"The Matrix","tmdbId":603,"year":1999})),
        )
        .mount(&radarr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/movie"))
        .and(header("x-api-key", "secret"))
        .and(body_partial_json(json!({
            "tmdbId":603,"title":"The Matrix","qualityProfileId":4,"rootFolderPath":"/data/root",
            "monitored":true,"minimumAvailability":"released","addOptions":{"searchForMovie":true}
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"id":42,"title":"The Matrix","tmdbId":603})),
        )
        .expect(1)
        .mount(&radarr)
        .await;

    let pool = pool().await;
    let sid = add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, mut rx) = service(&pool, Some(&tmdb), fast_cfg());

    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    assert_eq!(r.status, Searching);
    assert_eq!(r.arr_id, Some(sid));
    assert_eq!(r.arr_item_id, Some(42));
    assert_eq!(r.quality_profile_id, Some(4));
    assert_eq!(r.root_folder.as_deref(), Some("/data/root"));
    assert_eq!(r.title.as_deref(), Some("The Matrix"));
    assert_eq!(r.approved_by, Some(1));

    let evs = collect_until(&mut rx, r.id, Searching).await;
    assert_eq!(evs, vec![(Requested, 0), (Approved, 0), (Searching, 0)]);

    // Duplicate is rejected.
    let err = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));
    // Snapshot carries it.
    let snap = svc.active_snapshot().await.unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].state, Searching);
}

#[tokio::test]
async fn existing_movie_in_radarr_is_attached_and_searched() {
    let radarr = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .and(query_param("tmdbId", "603"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!([{"id":7,"title":"The Matrix","tmdbId":603,"monitored":true,"hasFile":false}]),
        ))
        .mount(&radarr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/command"))
        .and(body_partial_json(
            json!({"name":"MoviesSearch","movieIds":[7]}),
        ))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"id":1,"name":"MoviesSearch","status":"queued"})),
        )
        .expect(1)
        .mount(&radarr)
        .await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, _rx) = service(&pool, None, fast_cfg());
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    assert_eq!(r.status, Searching);
    assert_eq!(r.arr_item_id, Some(7));
}

#[tokio::test]
async fn quota_approval_decline_and_retry() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie/lookup/tmdb"))
        .and(query_param("tmdbId", "604"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"title":"B","tmdbId":604})))
        .mount(&radarr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/command"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"id":5,"name":"MoviesSearch","status":"queued"})),
        )
        .expect(1)
        .mount(&radarr)
        .await;

    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    sqlx::query("INSERT INTO user_limits (user_id, request_quota_movies, quota_window_days, auto_approve) VALUES (2, 2, 7, 0)")
        .execute(&pool)
        .await
        .unwrap();
    let (svc, _rx) = service(&pool, None, fast_cfg());

    // Non-auto-approve user stays `requested`, no Arr call yet.
    let a = svc
        .create_request(2, MediaType::Movie, 603, None)
        .await
        .unwrap();
    assert_eq!(a.status, Requested);
    let b = svc
        .create_request(2, MediaType::Movie, 604, None)
        .await
        .unwrap();
    assert_eq!(b.status, Requested);
    // Quota of 2 reached.
    let err = svc
        .create_request(2, MediaType::Movie, 605, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::QuotaExceeded(_)), "{err:?}");
    // Admin is exempt from quota.
    assert_eq!(
        svc.create_request(1, MediaType::Movie, 606, None)
            .await
            .unwrap()
            .status,
        Approved
    ); // no arr add mock for 606 -> stays approved w/ note

    // Approve → pushed.
    let a = svc.approve(a.id, 1).await.unwrap();
    assert_eq!(a.status, Searching);
    assert_eq!(a.approved_by, Some(1));
    // Decline frees the quota slot.
    let b = svc
        .decline(b.id, 1, Some("no thanks".into()))
        .await
        .unwrap();
    assert_eq!(b.status, Declined);
    assert_eq!(b.note.as_deref(), Some("no thanks"));
    assert!(svc.approve(b.id, 1).await.is_err());
    assert!(
        svc.create_request(2, MediaType::Movie, 605, None)
            .await
            .is_ok()
    );

    let pending = svc
        .list(&ListFilter {
            status: Some(Requested),
            user_id: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);

    // retry from searching triggers MoviesSearch (expect(1) on command mock).
    let r = svc.retry(a.id).await.unwrap();
    assert_eq!(r.status, Searching);
}

#[tokio::test]
async fn auto_approve_user() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    sqlx::query("INSERT INTO user_limits (user_id, auto_approve) VALUES (2, 1)")
        .execute(&pool)
        .await
        .unwrap();
    let (svc, _rx) = service(&pool, None, fast_cfg());
    let r = svc
        .create_request(2, MediaType::Movie, 603, None)
        .await
        .unwrap();
    assert_eq!(r.status, Searching);
}

#[tokio::test]
async fn availability_badges_and_mark_available() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies'),(2,'S','shows')")
        .execute(&pool)
        .await
        .unwrap();
    // movie 10 in library with a file; show 20 in library with an episode; show 21 empty shell.
    sqlx::query("INSERT INTO media_items (id, library_id, kind, title, tmdb_id) VALUES (1,1,'movie','A',10),(2,2,'show','S',20),(3,2,'season','S1',NULL),(4,2,'episode','E1',NULL),(5,2,'show','S2',21)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE media_items SET parent_id = 2 WHERE id = 3")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE media_items SET parent_id = 3 WHERE id = 4")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_files (item_id, path, size, mtime) VALUES (1,'/a.mkv',1,1)")
        .execute(&pool)
        .await
        .unwrap();
    let (svc, _rx) = service(&pool, None, fast_cfg());
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();
    assert_eq!(r.status, Searching);

    let m = svc
        .availability_for(MediaType::Movie, &[10, 603, 999])
        .await
        .unwrap();
    assert_eq!(m[&10], Availability::Available);
    assert_eq!(m[&603], Availability::Requested);
    assert_eq!(m[&999], Availability::None);

    // A tv request in flight for a show that's already partly in the library → partially.
    sqlx::query("INSERT INTO requests (user_id, media_type, tmdb_id, status) VALUES (1,'tv',20,'downloading')")
        .execute(&pool)
        .await
        .unwrap();
    let t = svc
        .availability_for(MediaType::Tv, &[20, 21, 22])
        .await
        .unwrap();
    assert_eq!(t[&20], Availability::Partially);
    assert_eq!(t[&21], Availability::Available);
    assert_eq!(t[&22], Availability::None);

    // Scanner glue.
    let flipped = svc.mark_available_by_tmdb(603).await.unwrap();
    assert_eq!(flipped, vec![r.id]);
    assert_eq!(svc.get(r.id).await.unwrap().status, Available);
    assert!(svc.mark_available_by_tmdb(603).await.unwrap().is_empty());
    // tv in `downloading` is not flipped by a first-episode ingest.
    assert!(svc.mark_available_by_tmdb(20).await.unwrap().is_empty());
}

#[tokio::test]
async fn webhook_transitions_and_paths() {
    let radarr = MockServer::start().await;
    mock_radarr_add(&radarr, 603, 42).await;
    let pool = pool().await;
    add_arr(&pool, "radarr", &radarr.uri()).await;
    let (svc, mut rx) = service(&pool, None, fast_cfg());
    let r = svc
        .create_request(1, MediaType::Movie, 603, None)
        .await
        .unwrap();

    let grab = json!({"eventType":"Grab","movie":{"id":42,"title":"The Matrix","tmdbId":603},
        "release":{"releaseTitle":"Matrix.1999","size":1000},"downloadId":"ABC"});
    let o = svc
        .handle_webhook(grab.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert_eq!(o.request_ids, vec![r.id]);
    assert_eq!(svc.get(r.id).await.unwrap().status, Queued);

    let dl = json!({"eventType":"Download","movie":{"id":42,"tmdbId":603,"folderPath":"/movies/The Matrix (1999)"},
        "movieFile":{"relativePath":"The Matrix.mkv"}});
    let o = svc
        .handle_webhook(dl.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert_eq!(
        o.imported_paths,
        vec!["/movies/The Matrix (1999)/The Matrix.mkv"]
    );
    assert_eq!(svc.get(r.id).await.unwrap().status, Importing);

    let failed =
        json!({"eventType":"DownloadFailed","movie":{"id":42,"tmdbId":603},"message":"bad nzb"});
    svc.handle_webhook(failed.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert_eq!(svc.get(r.id).await.unwrap().status, Failed);

    // Grab revives a failed request (Radarr re-searched).
    svc.handle_webhook(grab.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert_eq!(svc.get(r.id).await.unwrap().status, Queued);

    // Test event and unknown movie are harmless.
    svc.handle_webhook(br#"{"eventType":"Test"}"#, None)
        .await
        .unwrap();
    let other =
        json!({"eventType":"Download","movie":{"id":1,"tmdbId":1},"movieFile":{"path":"/x.mkv"}});
    let o = svc
        .handle_webhook(other.to_string().as_bytes(), None)
        .await
        .unwrap();
    assert!(o.request_ids.is_empty());
    assert_eq!(o.imported_paths, vec!["/x.mkv"]);
    // Wrong server id filters out.
    let o = svc
        .handle_webhook(dl.to_string().as_bytes(), Some(999))
        .await
        .unwrap();
    assert!(o.request_ids.is_empty());
    assert!(svc.handle_webhook(b"garbage", None).await.is_err());

    let mut states = vec![];
    while let Ok(reel_core::ServerEvent::RequestProgress { state, .. }) = rx.try_recv() {
        states.push(state);
    }
    assert_eq!(
        states,
        vec![
            Requested, Approved, Searching, Queued, Importing, Failed, Queued
        ]
    );
}

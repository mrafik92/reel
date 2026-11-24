//! Two logins on one Xtream provider (#297): one catalog, two connections.

use std::collections::HashMap;

use axum::{Router, extract::Query, http::header, response::IntoResponse, routing::get};
use reel_iptv::{
    dedup,
    hub::ChannelResolver,
    ingest::Ingest,
    store::{self, ChannelFilter, DbResolver},
    vod,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// The same panel for both logins; `u2`'s package also has one channel, movie and series more.
async fn api(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    let extra = q.get("username").map(String::as_str) == Some("u2");
    let mut body = match q.get("action").map(String::as_str) {
        None => {
            json!({"user_info":{"auth":1,"status":"Active","max_connections":"1","active_cons":"0"},
            "server_info":{"url":"x"}})
        }
        Some("get_live_categories") => json!([{"category_id":"1","category_name":"News"}]),
        Some("get_live_streams") => json!([
            {"num":1,"name":"News One","stream_id":101,"epg_channel_id":"","category_id":"1"},
            {"num":2,"name":"Sport Two","stream_id":102,"epg_channel_id":"","category_id":"1"}
        ]),
        Some("get_vod_categories") => json!([{"category_id":"10","category_name":"Drama"}]),
        Some("get_vod_streams") => json!([
            {"name":"Dune (2021)","stream_id":1001,"category_id":"10","container_extension":"mkv"},
            {"name":"Amélie","stream_id":1002,"category_id":"10","container_extension":"mp4"}
        ]),
        Some("get_series_categories") => json!([{"category_id":"20","category_name":"Shows"}]),
        Some("get_series") => json!([{"series_id":2001,"name":"Show One","category_id":"20"}]),
        _ => json!([]),
    };
    if extra {
        let push = |v: &mut Value, item: Value| v.as_array_mut().unwrap().push(item);
        match q.get("action").map(String::as_str) {
            Some("get_live_streams") => push(
                &mut body,
                json!({"num":3,"name":"Only Two","stream_id":103,"epg_channel_id":"","category_id":"1"}),
            ),
            Some("get_vod_streams") => push(
                &mut body,
                json!({"name":"Extra Film","stream_id":1003,"category_id":"10","container_extension":"mp4"}),
            ),
            Some("get_series") => push(
                &mut body,
                json!({"series_id":2002,"name":"Show Two","category_id":"20"}),
            ),
            _ => {}
        }
    }
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
}

async fn panel() -> String {
    let app = Router::new().route("/player_api.php", get(api));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

async fn login(pool: &SqlitePool, name: &str, url: &str, user: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password) \
         VALUES (?, 'xtream', ?, ?, 'pw') RETURNING id",
    )
    .bind(name)
    .bind(url)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn ids(pool: &SqlitePool, sql: &str) -> Vec<(i64, i64)> {
    sqlx::query_as(sql).fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn second_login_on_the_same_provider_adds_connections_not_copies() {
    let dir = tempfile::tempdir().unwrap();
    let base = panel().await;
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let ing = Ingest {
        pool: pool.clone(),
        http: reqwest::Client::new(),
        work_dir: dir.path().join("tmp"),
        epg_back_secs: 86_400,
        epg_fwd_secs: 86_400,
        allow_private: true,
        gates: reel_iptv::gate::Gates::new(Default::default()),
    };
    let s1 = login(&pool, "main", &base, "u1").await;
    // same server written differently: still the same provider
    let s2 = login(&pool, "extra", &format!("{base}/"), "u2").await;
    for s in [s1, s2] {
        ing.refresh_source(&store::get_source(&pool, s).await.unwrap())
            .await
            .unwrap();
    }

    // every channel once; the one only the second login has comes from it
    let chans = store::list_channels(&pool, &ChannelFilter::default())
        .await
        .unwrap();
    let mut seen: Vec<(String, i64)> = chans
        .iter()
        .map(|c| (c.name.clone(), c.source_id))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("News One".into(), s1),
            ("Only Two".into(), s2),
            ("Sport Two".into(), s1)
        ]
    );
    assert_eq!(
        store::list_groups(&pool).await.unwrap(),
        [("News".into(), 3)]
    );
    // the admin view still lists every login's rows
    let all = store::list_channels(
        &pool,
        &ChannelFilter {
            include_hidden: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(all.len(), 5);

    // VOD: one row per title is visible
    let visible_movies = ids(
        &pool,
        "SELECT stream_id, source_id FROM vod_movies WHERE missing = 0 AND duplicate = 0 ORDER BY 1",
    )
    .await;
    assert_eq!(visible_movies, [(1001, s1), (1002, s1), (1003, s2)]);
    let visible_series = ids(
        &pool,
        "SELECT series_id, source_id FROM vod_series WHERE missing = 0 AND duplicate = 0 ORDER BY 1",
    )
    .await;
    assert_eq!(visible_series, [(2001, s1), (2002, s2)]);

    // a channel resolves to both logins, the main one first
    let news = chans.iter().find(|c| c.name == "News One").unwrap();
    let spec = DbResolver { pool: pool.clone() }
        .resolve(news.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(spec.candidates.len(), 2);
    assert!(spec.candidates[0].url.ends_with("/live/u1/pw/101.ts"));
    assert!(spec.candidates[1].url.ends_with("/live/u2/pw/101.ts"));
    assert_eq!(
        spec.candidates
            .iter()
            .map(|c| c.source_id)
            .collect::<Vec<_>>(),
        [s1, s2]
    );

    // a movie plays through either login; a title only one login has, through that one
    let dune: i64 =
        sqlx::query_scalar("SELECT id FROM vod_movies WHERE stream_id = 1001 AND source_id = ?")
            .bind(s1)
            .fetch_one(&pool)
            .await
            .unwrap();
    let t = vod::resolve_targets(&pool, "movie", dune).await.unwrap();
    assert_eq!(t.iter().map(|t| t.source_id).collect::<Vec<_>>(), [s1, s2]);
    assert!(t[0].url.ends_with("/movie/u1/pw/1001.mkv"), "{}", t[0].url);
    assert!(t[1].url.ends_with("/movie/u2/pw/1001.mkv"), "{}", t[1].url);
    let extra: i64 = sqlx::query_scalar("SELECT id FROM vod_movies WHERE stream_id = 1003")
        .fetch_one(&pool)
        .await
        .unwrap();
    let t = vod::resolve_targets(&pool, "movie", extra).await.unwrap();
    assert_eq!(t.iter().map(|t| t.source_id).collect::<Vec<_>>(), [s2]);

    // progress and favourites saved on a copy move to the visible row
    let dune2: i64 =
        sqlx::query_scalar("SELECT id FROM vod_movies WHERE stream_id = 1001 AND source_id = ?")
            .bind(s2)
            .fetch_one(&pool)
            .await
            .unwrap();
    let news2: i64 =
        sqlx::query_scalar("SELECT id FROM channels WHERE ext_id = '101' AND source_id = ?")
            .bind(s2)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO vod_watch (user_id, kind, item_id, position_s, last_played_at) \
         VALUES (1, 'movie', ?, 600, 200), (1, 'movie', ?, 30, 100)",
    )
    .bind(dune2)
    .bind(dune)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_channel_favorites (user_id, channel_id) VALUES (1, ?)")
        .bind(news2)
        .execute(&pool)
        .await
        .unwrap();
    let rep = dedup::refresh(&pool).await.unwrap();
    assert_eq!(rep.moved, 2);
    let watch: Vec<(i64, f64)> = sqlx::query_as("SELECT item_id, position_s FROM vod_watch")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(watch, [(dune, 600.0)], "newer progress wins");
    let favs: Vec<i64> = sqlx::query_scalar("SELECT channel_id FROM user_channel_favorites")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(favs, [news.id]);

    // the main login disabled: the other one's rows show instead, and playback uses it alone
    sqlx::query("UPDATE iptv_sources SET enabled = 0 WHERE id = ?")
        .bind(s1)
        .execute(&pool)
        .await
        .unwrap();
    dedup::refresh(&pool).await.unwrap();
    let visible_movies = ids(
        &pool,
        "SELECT stream_id, source_id FROM vod_movies WHERE missing = 0 AND duplicate = 0 ORDER BY 1",
    )
    .await;
    assert_eq!(visible_movies, [(1001, s2), (1002, s2), (1003, s2)]);
    let chans = store::list_channels(&pool, &ChannelFilter::default())
        .await
        .unwrap();
    assert_eq!(chans.len(), 3);
    assert!(chans.iter().all(|c| c.source_id == s2));
    let t = vod::resolve_targets(&pool, "movie", dune2).await.unwrap();
    assert_eq!(t.iter().map(|t| t.source_id).collect::<Vec<_>>(), [s2]);

    // a login on another server is its own provider
    sqlx::query(
        "UPDATE iptv_sources SET enabled = 1, url = 'http://other.example:8080' WHERE id = ?",
    )
    .bind(s1)
    .execute(&pool)
    .await
    .unwrap();
    dedup::refresh(&pool).await.unwrap();
    let dups: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM channels WHERE duplicate = 1) \
              + (SELECT count(*) FROM vod_movies WHERE duplicate = 1) \
              + (SELECT count(*) FROM vod_series WHERE duplicate = 1)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(dups, 0);
    assert!(dedup::mirrors(&pool, s2).await.unwrap().is_empty());
}

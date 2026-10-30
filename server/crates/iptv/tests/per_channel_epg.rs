//! Xtream panels without an XMLTV feed: lazy `get_short_epg` per channel, base64 payloads,
//! politeness (rate limit, 46x backoff, in-flight de-duplication).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use base64::Engine;
use reel_iptv::{
    epg_fetch::EpgFetcher,
    gate::{GateConfig, Gates},
    ingest::Ingest,
    store,
};
use serde_json::json;
use sqlx::SqlitePool;

#[derive(Default)]
struct Prov {
    short_epg_calls: AtomicUsize,
    other_calls: AtomicUsize,
    /// 0 = serve data
    block_with: AtomicU16,
    calls: Mutex<Vec<(String, Instant)>>,
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

async fn api(
    State(p): State<Arc<Prov>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let action = q.get("action").cloned().unwrap_or_default();
    p.calls
        .lock()
        .unwrap()
        .push((action.clone(), Instant::now()));
    if action == "get_short_epg" {
        p.short_epg_calls.fetch_add(1, SeqCst);
        let code = p.block_with.load(SeqCst);
        if code != 0 {
            return StatusCode::from_u16(code).unwrap().into_response();
        }
        let id: i64 = q["stream_id"].parse().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        return axum::Json(json!({"epg_listings": [
            {"id":"1","epg_id":"x","title": b64(&format!("Now on {id}")), "lang":"",
             "start":"2026-01-01 10:00:00","end":"2026-01-01 11:00:00",
             "description": b64("Ünïcode description"), "channel_id":"c",
             "start_timestamp": (now - 600).to_string(), "stop_timestamp": (now + 1800).to_string()},
            {"id":"2","epg_id":"x","title": b64("Next Show"), "lang":"", "start":"","end":"",
             "description":"", "channel_id":"c",
             "start_timestamp": (now + 1800).to_string(), "stop_timestamp": (now + 5400).to_string()}
        ]}))
        .into_response();
    }
    p.other_calls.fetch_add(1, SeqCst);
    let body = match action.as_str() {
        "get_live_categories" => json!([{"category_id":"1","category_name":"General"}]),
        "get_live_streams" => json!([
            {"num":1,"name":"One","stream_id":101,"stream_icon":"","epg_channel_id":"","category_id":"1"},
            {"num":2,"name":"Two","stream_id":102,"stream_icon":"","epg_channel_id":"two.xx","category_id":"1"},
            {"num":3,"name":"Three","stream_id":103,"stream_icon":"","epg_channel_id":"","category_id":"1"}
        ]),
        "get_vod_categories" | "get_series_categories" | "get_vod_streams" | "get_series" => {
            json!([])
        }
        _ => {
            json!({"user_info":{"auth":1,"status":"Active","max_connections":"1"},"server_info":{"url":"x"}})
        }
    };
    axum::Json(body).into_response()
}

async fn xmltv() -> impl IntoResponse {
    // the provider has no guide: HTTP 200 with an empty <tv/>
    (
        [("content-type", "text/xml")],
        "<?xml version=\"1.0\"?><tv generator-info-name=\"x\"></tv>",
    )
}

async fn setup(
    gate: GateConfig,
) -> (
    Arc<Prov>,
    SqlitePool,
    i64,
    Ingest,
    Arc<EpgFetcher>,
    Arc<Gates>,
    tempfile::TempDir,
) {
    let prov = Arc::new(Prov::default());
    let app = Router::new()
        .route("/player_api.php", get(api))
        .route("/xmltv.php", get(xmltv))
        .with_state(prov.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password) VALUES ('P','xtream',?,'u','p') RETURNING id",
    )
    .bind(url)
    .fetch_one(&pool)
    .await
    .unwrap();
    let gates = Gates::new(gate);
    let http = reqwest::Client::new();
    let ing = Ingest {
        pool: pool.clone(),
        http: http.clone(),
        work_dir: dir.path().join("tmp"),
        epg_back_secs: 86_400,
        epg_fwd_secs: 7 * 86_400,
        allow_private: true,
        gates: gates.clone(),
    };
    let fetcher = EpgFetcher::new(pool.clone(), http, gates.clone());
    (prov, pool, sid, ing, fetcher, gates, dir)
}

async fn channel(pool: &SqlitePool, ext: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM channels WHERE ext_id = ?")
        .bind(ext)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn empty_xmltv_switches_to_per_channel_and_programmes_are_fetched_lazily_and_cached() {
    let (prov, pool, sid, ing, fetcher, _g, _d) = setup(GateConfig::default()).await;
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_source(&src).await.unwrap();
    assert!(r.epg_error.is_some());
    let s = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(s.epg_mode, "per_channel");
    assert!(
        s.epg_note
            .unwrap()
            .contains("no channels and no programmes")
    );
    assert!(
        s.last_error.is_none(),
        "an empty guide is not an error when the fallback works"
    );
    // nothing is fetched at refresh time: it is lazy
    assert_eq!(prov.short_epg_calls.load(SeqCst), 0);

    let (c1, c2, c3) = (
        channel(&pool, "101").await,
        channel(&pool, "102").await,
        channel(&pool, "103").await,
    );
    // only the requested channel is fetched
    fetcher.ensure(&[c1], Duration::from_secs(3)).await;
    assert_eq!(prov.short_epg_calls.load(SeqCst), 1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let map = store::now_next(&pool, &[c1, c2, c3], now).await.unwrap();
    let (n, x) = map.get(&c1).cloned().unwrap();
    assert_eq!(n.unwrap().title, "Now on 101", "base64 title decoded");
    assert_eq!(x.unwrap().title, "Next Show");
    assert!(
        map.get(&c2).is_none_or(|(n, _)| n.is_none()),
        "other channels untouched"
    );
    let (desc,): (Option<String>,) =
        sqlx::query_as("SELECT description FROM epg_programs WHERE title = 'Now on 101'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(desc.as_deref(), Some("Ünïcode description"));
    // synthetic guide key for a channel without tvg-id; the provider's id when it has one
    let key: Option<String> =
        sqlx::query_scalar("SELECT epg_channel_id FROM channels WHERE id = ?")
            .bind(c1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(key.as_deref(), Some(format!("xc:{sid}:101").as_str()));

    // fresh data is not fetched again; the second channel is
    fetcher.ensure(&[c1, c2], Duration::from_secs(3)).await;
    assert_eq!(prov.short_epg_calls.load(SeqCst), 2);
    let k2: Option<String> = sqlx::query_scalar("SELECT epg_channel_id FROM channels WHERE id = ?")
        .bind(c2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(k2.as_deref(), Some("two.xx"));
    // counts for the admin DTO
    let s = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(s.epg_programme_count, 4);
    assert_eq!(s.epg_matched_channels, 2);

    // concurrent requests for the same channel share one fetch
    let c = c3;
    let ids = [c];
    tokio::join!(
        fetcher.ensure(&ids, Duration::from_secs(3)),
        fetcher.ensure(&ids, Duration::from_secs(3))
    );
    assert_eq!(prov.short_epg_calls.load(SeqCst), 3);
}

#[tokio::test]
async fn per_channel_fetching_is_rate_limited_and_backs_off_on_46x() {
    let (prov, pool, sid, ing, fetcher, gates, _d) = setup(GateConfig {
        rate_per_sec: 10.0,
        burst: 2.0,
        backoff_base: Duration::from_millis(400),
        backoff_max: Duration::from_secs(5),
    })
    .await;
    let src = store::get_source(&pool, sid).await.unwrap();
    ing.refresh_source(&src).await.unwrap();
    let ids = [
        channel(&pool, "101").await,
        channel(&pool, "102").await,
        channel(&pool, "103").await,
    ];
    // pacing: a burst of 2, then 10/s
    let t = Instant::now();
    fetcher.ensure(&ids, Duration::from_secs(5)).await;
    assert_eq!(prov.short_epg_calls.load(SeqCst), 3);
    assert!(
        t.elapsed() >= Duration::from_millis(80),
        "{:?}",
        t.elapsed()
    );

    // the panel starts answering 469: the first rejection opens the breaker, nothing more is sent
    sqlx::query("DELETE FROM epg_fetch_state")
        .execute(&pool)
        .await
        .unwrap();
    prov.block_with.store(469, SeqCst);
    let before = prov.short_epg_calls.load(SeqCst);
    fetcher.ensure(&ids, Duration::from_secs(3)).await;
    assert_eq!(
        prov.short_epg_calls.load(SeqCst),
        before + 1,
        "stopped after the first 469"
    );
    let gate = gates.get(sid);
    assert!(gate.penalty_remaining().is_some());
    assert_eq!(gate.last_status(), Some(469));
    // while the breaker is open nobody calls the provider (not even the catalog)
    fetcher.ensure(&ids, Duration::from_secs(1)).await;
    assert_eq!(prov.short_epg_calls.load(SeqCst), before + 1);
    // …and the data fetched before is still served from the cache
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        store::now_next(&pool, &ids, now)
            .await
            .unwrap()
            .values()
            .any(|(n, _)| n.is_some())
    );
    // after the pause the provider recovers and fetching resumes
    prov.block_with.store(0, SeqCst);
    tokio::time::sleep(Duration::from_millis(450)).await;
    fetcher.ensure(&ids, Duration::from_secs(3)).await;
    assert!(prov.short_epg_calls.load(SeqCst) > before + 1);
}

#[tokio::test]
async fn catalog_refresh_waits_out_a_provider_block_instead_of_failing() {
    let (prov, pool, sid, ing, _f, gates, _d) = setup(GateConfig {
        backoff_base: Duration::from_millis(300),
        ..Default::default()
    })
    .await;
    gates.get(sid).report(469, None);
    let t = Instant::now();
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_channels(&src).await.unwrap();
    assert_eq!(r.total, 3);
    assert!(
        t.elapsed() >= Duration::from_millis(250),
        "waited for the backoff: {:?}",
        t.elapsed()
    );
    assert!(prov.other_calls.load(SeqCst) >= 3);
}

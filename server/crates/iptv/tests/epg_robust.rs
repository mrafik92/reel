//! Real-world guide problems seen with Xtream providers: gzip, timezone offsets, HTML/empty
//! bodies, picky User-Agents, missing tvg-ids.

use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use reel_iptv::{ingest::Ingest, store};
use sqlx::SqlitePool;

#[derive(Clone, Default)]
struct Provider {
    /// Body served at `/xmltv.php`.
    guide: Arc<Mutex<Vec<u8>>>,
    /// Content type of the guide.
    ctype: Arc<Mutex<&'static str>>,
    /// User-Agents seen on every request.
    uas: Arc<Mutex<Vec<String>>>,
}

async fn api(State(p): State<Provider>, h: HeaderMap, q: axum::extract::RawQuery) -> Response {
    let ua = h
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    p.uas.lock().unwrap().push(ua.clone());
    // picky provider: only player-like agents are served
    if !(ua.starts_with("VLC/") || ua.starts_with("MyPlayer/")) {
        return (StatusCode::FORBIDDEN, "<html>blocked</html>").into_response();
    }
    let q = q.0.unwrap_or_default();
    let body = if q.contains("action=get_live_categories") {
        r#"[{"category_id":"1","category_name":"AR | General"},{"category_id":"2","category_name":"UK | News"}]"#
    } else if q.contains("action=get_live_streams") {
        r#"[
          {"num":1,"name":"AR: MBC 1 HD","stream_id":101,"stream_icon":"","epg_channel_id":"mbc1.ae","category_id":"1","tv_archive":0},
          {"num":2,"name":"AR: MBC 2 FHD HEVC M","stream_id":102,"stream_icon":"","epg_channel_id":"","category_id":"1","tv_archive":0},
          {"num":3,"name":"UK| Sky News UHD","stream_id":103,"stream_icon":"","epg_channel_id":null,"category_id":"2","tv_archive":0},
          {"num":4,"name":"Mystery Channel","stream_id":104,"stream_icon":"","epg_channel_id":"","category_id":"2","tv_archive":0}
        ]"#
    } else {
        r#"{"user_info":{"auth":1,"status":"Active","max_connections":"1","active_cons":"0","is_trial":"0","exp_date":"1999999999"},"server_info":{"url":"x"}}"#
    };
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn xmltv(State(p): State<Provider>, h: HeaderMap) -> Response {
    let ua = h
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    p.uas.lock().unwrap().push(ua.clone());
    if !(ua.starts_with("VLC/") || ua.starts_with("MyPlayer/")) {
        return (StatusCode::FORBIDDEN, "<html>blocked</html>").into_response();
    }
    let body = p.guide.lock().unwrap().clone();
    let ct = *p.ctype.lock().unwrap();
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}

async fn start() -> (Provider, String) {
    let p = Provider::default();
    *p.ctype.lock().unwrap() = "text/xml";
    let app = Router::new()
        .route("/player_api.php", get(api))
        .route("/xmltv.php", get(xmltv))
        .with_state(p.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (p, url)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `YYYYMMDDHHMMSS ±HHMM` for `unix` as seen in a zone `offset_min` minutes from UTC.
fn xt(unix: i64, offset_min: i64) -> String {
    let local = unix + offset_min * 60;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let sign = if offset_min >= 0 { '+' } else { '-' };
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} {sign}{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        offset_min.abs() / 60,
        offset_min.abs() % 60
    )
}

fn guide(base: i64) -> String {
    let p = |ch: &str, from: i64, to: i64, tz: i64, title: &str| {
        format!(
            "<programme start=\"{}\" stop=\"{}\" channel=\"{ch}\"><title>{title}</title></programme>",
            xt(base + from, tz),
            xt(base + to, tz)
        )
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><tv generator-info-name=\"prov\">\
         <channel id=\"mbc1.ae\"><display-name>MBC 1</display-name></channel>\
         <channel id=\"mbc2.ae\"><display-name>MBC 2</display-name></channel>\
         <channel id=\"skynews.uk\"><display-name>Sky News</display-name></channel>\
         {}{}{}{}</tv>",
        p("mbc1.ae", -600, 3000, 120, "Arabic Morning"), // +0200 stamps
        p("mbc2.ae", -600, 3000, -300, "Movie Night"),   // -0500 stamps
        p("skynews.uk", -600, 3000, 0, "Headlines"),
        p("mbc1.ae", 3000, 6600, 120, "Next Show"),
    )
}

fn gz(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(b).unwrap();
    e.finish().unwrap()
}

async fn setup(url: &str, ua: Option<&str>) -> (SqlitePool, i64, Ingest, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password, user_agent) \
         VALUES ('Prov','xtream',?, 'bob', 'p@ss word', ?) RETURNING id",
    )
    .bind(url)
    .bind(ua)
    .fetch_one(&pool)
    .await
    .unwrap();
    let ing = Ingest {
        pool: pool.clone(),
        // a bare client: the default player User-Agent must come from the ingest code
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        work_dir: dir.path().join("tmp"),
        epg_back_secs: 86_400,
        epg_fwd_secs: 7 * 86_400,
        allow_private: true,
        gates: reel_iptv::gate::Gates::new(Default::default()),
    };
    (pool, sid, ing, dir)
}

async fn tvg(pool: &SqlitePool, ext: &str) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT tvg_id, epg_channel_id FROM channels WHERE ext_id = ?")
        .bind(ext)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn xtream_epg_ids_are_stored_gz_guide_with_zones_is_imported_and_names_fill_the_gaps() {
    let (prov, url) = start().await;
    let base = now() / 600 * 600;
    *prov.guide.lock().unwrap() = gz(guide(base).as_bytes());
    let (pool, sid, ing, _d) = setup(&url, None).await;
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_source(&src).await.unwrap();
    assert!(r.epg_error.is_none(), "{:?}", r.epg_error);
    assert_eq!(r.channels.total, 4);

    // Xtream's epg_channel_id is the channel's tvg id (empty/null → none)
    assert_eq!(tvg(&pool, "101").await.0.as_deref(), Some("mbc1.ae"));
    assert_eq!(tvg(&pool, "102").await.0, None);
    assert_eq!(tvg(&pool, "103").await.0, None);
    // exact id match, and name fallback after stripping "AR:", "UK|", quality + "M" suffixes
    assert_eq!(tvg(&pool, "101").await.1.as_deref(), Some("mbc1.ae"));
    assert_eq!(
        tvg(&pool, "102").await.1.as_deref(),
        Some("mbc2.ae"),
        "name fallback"
    );
    assert_eq!(
        tvg(&pool, "103").await.1.as_deref(),
        Some("skynews.uk"),
        "name fallback"
    );
    assert_eq!(tvg(&pool, "104").await.1, None, "nothing to match");

    let epg = r.epg.unwrap();
    assert_eq!(
        (
            epg.epg_channels,
            epg.programmes_seen,
            epg.programmes_inserted
        ),
        (3, 4, 4)
    );
    // timezone offsets are honoured: +0200 / -0500 / +0000 stamps all land on the same instants
    let rows: Vec<(String, i64, i64, String)> = sqlx::query_as(
        "SELECT channel_tvg_id, start, stop, title FROM epg_programs ORDER BY title",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let want =
        |ch: &str, a: i64, b: i64, t: &str| (ch.to_string(), base + a, base + b, t.to_string());
    assert_eq!(
        rows,
        vec![
            want("mbc1.ae", -600, 3000, "Arabic Morning"),
            want("skynews.uk", -600, 3000, "Headlines"),
            want("mbc2.ae", -600, 3000, "Movie Night"),
            want("mbc1.ae", 3000, 6600, "Next Show"),
        ]
    );

    // statistics for the admin UI
    let s = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(
        (
            s.epg_channel_count,
            s.epg_programme_count,
            s.epg_matched_channels
        ),
        (3, 4, 3)
    );
    assert!(s.last_epg_at.is_some() && s.last_error.is_none());
    // the Xtream API calls and the guide all went out with a player User-Agent
    let uas = prov.uas.lock().unwrap().clone();
    assert!(
        uas.len() >= 4 && uas.iter().all(|u| u == "VLC/3.0.20 LibVLC/3.0.20"),
        "{uas:?}"
    );
}

#[tokio::test]
async fn a_source_user_agent_overrides_the_default_everywhere() {
    let (prov, url) = start().await;
    *prov.guide.lock().unwrap() = guide(now() / 600 * 600).into_bytes();
    let (pool, sid, ing, _d) = setup(&url, Some("MyPlayer/1.0")).await;
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_source(&src).await.unwrap();
    assert!(r.epg_error.is_none(), "{:?}", r.epg_error);
    let uas = prov.uas.lock().unwrap().clone();
    assert!(uas.iter().all(|u| u == "MyPlayer/1.0"), "{uas:?}");
}

#[tokio::test]
async fn bad_guides_are_reported_never_as_success() {
    let (prov, url) = start().await;
    let (pool, sid, ing, _d) = setup(&url, None).await;
    let base = now() / 600 * 600;
    let cases: Vec<(Vec<u8>, &str, &str)> = vec![
        (b"<html><body>Login required</body></html>".to_vec(), "text/html", "HTML page"),
        (b"".to_vec(), "text/xml", "empty"),
        (b"{\"error\":\"bad\"}".to_vec(), "application/json", "JSON"),
        (gz(b"<html>nope</html>"), "application/gzip", "HTML page"),
        (b"<?xml version=\"1.0\"?><tv></tv>".to_vec(), "text/xml", "no channels and no programmes"),
        (
            b"<?xml version=\"1.0\"?><tv><channel id=\"a\"><display-name>A</display-name></channel></tv>".to_vec(),
            "text/xml",
            "0 programmes",
        ),
        // programmes only for channels we do not have
        (
            format!(
                "<?xml version=\"1.0\"?><tv><channel id=\"zzz\"><display-name>Zzz</display-name></channel>\
                 <programme start=\"{0}\" stop=\"{1}\" channel=\"zzz\"><title>X</title></programme></tv>",
                xt(base, 0),
                xt(base + 3600, 0)
            )
            .into_bytes(),
            "text/xml",
            "none belong to your channels",
        ),
    ];
    let src = store::get_source(&pool, sid).await.unwrap();
    ing.refresh_channels(&src).await.unwrap();
    for (body, ct, expect) in cases {
        *prov.guide.lock().unwrap() = body;
        *prov.ctype.lock().unwrap() = ct;
        let src = store::get_source(&pool, sid).await.unwrap();
        let err = ing
            .refresh_epg_recorded(&src)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(expect), "expected '{expect}' in: {err}");
        let s = store::get_source(&pool, sid).await.unwrap();
        // Xtream: an unusable guide is not an error, the lazy per-channel guide takes over
        assert!(s.last_error.is_none(), "{:?}", s.last_error);
        assert_eq!(s.epg_mode, "per_channel");
        assert!(
            s.epg_note.as_deref().unwrap_or_default().contains(expect),
            "{:?}",
            s.epg_note
        );
        assert!(
            s.last_epg_at.is_none(),
            "no success timestamp for a useless guide"
        );
    }
    // a good guide afterwards: re-running only the EPG clears the EPG error
    *prov.guide.lock().unwrap() = guide(base).into_bytes();
    *prov.ctype.lock().unwrap() = "text/xml";
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_epg_recorded(&src).await.unwrap();
    assert_eq!(r.programmes_inserted, 4);
    let s = store::get_source(&pool, sid).await.unwrap();
    assert!(s.last_error.is_none() && s.last_epg_at.is_some());
    assert_eq!((s.epg_mode.as_str(), s.epg_note), ("xmltv", None));
}

#[tokio::test]
async fn blocked_user_agent_error_has_no_credentials() {
    let (_prov, url) = start().await;
    let (pool, sid, ing, _d) = setup(&url, Some("curl/8")).await;
    let src = store::get_source(&pool, sid).await.unwrap();
    let e = ing.refresh_source(&src).await.unwrap_err().to_string();
    assert!(e.contains("403"), "{e}");
    let s = store::get_source(&pool, sid).await.unwrap();
    let le = s.last_error.unwrap();
    for leak in ["p%40ss", "p@ss", "bob"] {
        assert!(
            !le.contains(leak) && !e.contains(leak),
            "{leak} leaked: {le} / {e}"
        );
    }
}

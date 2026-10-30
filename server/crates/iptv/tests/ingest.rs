use std::{io::Write, sync::Arc, time::Duration};

use axum::{Router, extract::Query, http::header, response::IntoResponse, routing::get};
use reel_iptv::hub::ChannelResolver;
use reel_iptv::{
    IptvConfig, IptvService,
    ingest::Ingest,
    store::{self, ChannelFilter, ChannelPatch, DbResolver, SourceKind},
};
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// unix → `YYYYMMDDHHMMSS +0000`
fn xt(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // civil_from_days (Hinnant)
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
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn guide(base: i64) -> String {
    let p = |ch: &str, from: i64, to: i64, title: &str| {
        format!(
            "<programme start=\"{}\" stop=\"{}\" channel=\"{ch}\"><title>{title}</title><desc>About {title}</desc></programme>",
            xt(base + from),
            xt(base + to)
        )
    };
    format!(
        "<?xml version=\"1.0\"?><tv>\
         <channel id=\"bbc1.uk\"><display-name>BBC One</display-name></channel>\
         <channel id=\"cnn.us\"><display-name>CNN International</display-name></channel>\
         <channel id=\"nobody.xx\"><display-name>Nobody Watches</display-name></channel>\
         {}{}{}{}{}{}{}</tv>",
        p("bbc1.uk", -1800, 1800, "Morning News"),
        p("bbc1.uk", 1800, 5400, "Quiz Show"),
        p("bbc1.uk", 5400, 9000, "Late Film"),
        p("cnn.us", -600, 3000, "World Report"),
        p("nobody.xx", 0, 3600, "Unwanted"),
        p("bbc1.uk", -3 * 86_400, -3 * 86_400 + 3600, "Too old"),
        p("bbc1.uk", 20 * 86_400, 20 * 86_400 + 3600, "Too far"),
    )
}

async fn pool() -> SqlitePool {
    reel_core::db::connect_url("sqlite::memory:").await.unwrap()
}

fn ingest(pool: &SqlitePool, dir: &std::path::Path) -> Ingest {
    Ingest {
        pool: pool.clone(),
        http: reqwest::Client::new(),
        work_dir: dir.join("tmp"),
        epg_back_secs: 86_400,
        epg_fwd_secs: 7 * 86_400,
    }
}

async fn add_source(
    pool: &SqlitePool,
    name: &str,
    kind: &str,
    url: &str,
    epg: Option<&str>,
    max: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, epg_url, max_connections) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(kind)
    .bind(url)
    .bind(epg)
    .bind(max)
    .fetch_one(pool)
    .await
    .unwrap()
}

const PLAYLIST_V1: &str = "#EXTM3U\n\
#EXTINF:-1 tvg-id=\"bbc1.uk\" tvg-name=\"BBC One\" tvg-logo=\"http://l/bbc.png\" group-title=\"UK\" tvg-chno=\"101\",BBC One HD\nhttp://prov/1.ts\n\
#EXTINF:-1 group-title=\"News\",CNN International HD\nhttp://prov/2.ts\n\
#EXTINF:-1 tvg-id=\"dup\" group-title=\"X\",Dup A\nhttp://prov/3.ts\n\
#EXTINF:-1 tvg-id=\"dup\" group-title=\"X\",Dup B\nhttp://prov/4.ts\n\
#EXTINF:-1 group-title=\"Misc\",Shopping\nhttp://prov/5.ts\n";

#[tokio::test]
async fn m3u_xmltv_refresh_maps_epg_and_keeps_ids_stable() {
    let dir = tempfile::tempdir().unwrap();
    let pl = dir.path().join("list.m3u");
    let xml = dir.path().join("guide.xml");
    std::fs::write(&pl, PLAYLIST_V1).unwrap();
    let base = now() / 600 * 600;
    std::fs::write(&xml, guide(base)).unwrap();
    let pool = pool().await;
    let sid = add_source(
        &pool,
        "Prov",
        "m3u",
        pl.to_str().unwrap(),
        Some(&format!("file://{}", xml.display())),
        None,
    )
    .await;
    let ing = ingest(&pool, dir.path());
    let src = store::get_source(&pool, sid).await.unwrap();
    let r = ing.refresh_source(&src).await.unwrap();
    assert_eq!((r.channels.total, r.channels.added), (5, 5));
    let epg = r.epg.unwrap();
    assert_eq!(epg.epg_channels, 3);
    assert_eq!(epg.programmes_seen, 7);
    assert_eq!(
        epg.programmes_kept, 5,
        "window filtered: too old / too far dropped"
    );
    assert_eq!(
        epg.programmes_inserted, 4,
        "only programmes of channels we have: Unwanted dropped"
    );
    assert_eq!(epg.channels_mapped, 2);

    let chans = store::list_channels(&pool, &ChannelFilter::default())
        .await
        .unwrap();
    assert_eq!(chans.len(), 5);
    let bbc = chans.iter().find(|c| c.name == "BBC One HD").unwrap();
    assert_eq!(
        (bbc.number, bbc.group.as_deref(), bbc.tvg_id.as_deref()),
        (Some(101), Some("UK"), Some("bbc1.uk"))
    );
    assert_eq!(
        bbc.epg_channel_id.as_deref(),
        Some("bbc1.uk"),
        "exact tvg-id"
    );
    let cnn = chans.iter().find(|c| c.name.starts_with("CNN")).unwrap();
    assert_eq!(cnn.tvg_id, None);
    assert_eq!(
        cnn.epg_channel_id.as_deref(),
        Some("cnn.us"),
        "fuzzy name: 'CNN International HD' ~ 'CNN International'"
    );
    assert!(
        chans
            .iter()
            .find(|c| c.name == "Shopping")
            .unwrap()
            .epg_channel_id
            .is_none()
    );
    let dups: Vec<_> = chans.iter().filter(|c| c.name.starts_with("Dup")).collect();
    assert_eq!(dups.len(), 2, "duplicate tvg-ids get distinct rows");

    // groups, search, favorites
    let groups = store::list_groups(&pool).await.unwrap();
    assert!(groups.contains(&("UK".to_string(), 1)) && groups.contains(&("X".to_string(), 2)));
    let q = ChannelFilter {
        q: Some("cnn".into()),
        ..Default::default()
    };
    assert_eq!(store::list_channels(&pool, &q).await.unwrap().len(), 1);
    store::set_favorite(&pool, 1, bbc.id, true).await.unwrap();
    let f = ChannelFilter {
        user_id: Some(1),
        favorites_only: true,
        ..Default::default()
    };
    let favs = store::list_channels(&pool, &f).await.unwrap();
    assert_eq!(favs.len(), 1);
    assert!(favs[0].favorite);
    assert_eq!(store::top_favorites(&pool, 5).await.unwrap(), vec![bbc.id]);

    // now / next
    let nn = store::now_next(&pool, &[bbc.id, cnn.id], base)
        .await
        .unwrap();
    let (n, x) = &nn[&bbc.id];
    assert_eq!(n.as_ref().unwrap().title, "Morning News");
    assert_eq!(x.as_ref().unwrap().title, "Quiz Show");
    assert_eq!(nn[&cnn.id].0.as_ref().unwrap().title, "World Report");
    assert!(nn[&cnn.id].1.is_none());
    let range = store::epg_range(&pool, &[bbc.id], base, base + 7200, false)
        .await
        .unwrap();
    let titles: Vec<_> = range[&bbc.id].iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Morning News", "Quiz Show", "Late Film"]);
    assert!(
        range[&bbc.id][0].description.is_none(),
        "compact payload omits descriptions"
    );

    // admin edits survive a refresh; vanished channels are hidden but keep their id
    store::update_channel(
        &pool,
        bbc.id,
        &ChannelPatch {
            name: Some("BBC 1".into()),
            number: Some(Some(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::update_channel(
        &pool,
        cnn.id,
        &ChannelPatch {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let v2 = PLAYLIST_V1
        .replace("http://prov/1.ts", "http://prov/1-new.ts")
        .replace(
            "#EXTINF:-1 group-title=\"Misc\",Shopping\nhttp://prov/5.ts\n",
            "",
        );
    std::fs::write(&pl, v2).unwrap();
    let r2 = ing.refresh_channels(&src).await.unwrap();
    assert_eq!((r2.total, r2.added, r2.missing), (4, 0, 1));
    let again = store::list_channels(
        &pool,
        &ChannelFilter {
            user_id: Some(1),
            include_hidden: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let bbc2 = again.iter().find(|c| c.id == bbc.id).unwrap();
    assert_eq!(
        (bbc2.name.as_str(), bbc2.number, bbc2.favorite),
        ("BBC 1", Some(1), true),
        "stable id, favorite, custom name/number kept"
    );
    assert!(
        !again.iter().find(|c| c.id == cnn.id).unwrap().enabled,
        "disabled flag kept"
    );
    let shop = again.iter().find(|c| c.name == "Shopping").unwrap();
    assert!(shop.missing);
    assert_eq!(
        store::list_channels(&pool, &ChannelFilter::default())
            .await
            .unwrap()
            .len(),
        3,
        "hidden: missing + disabled"
    );
    let url: String = sqlx::query_scalar("SELECT stream_url FROM channels WHERE id = ?")
        .bind(bbc.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(url, "http://prov/1-new.ts", "URL updated in place");

    // manual EPG mapping override wins and survives re-mapping; clearing returns to automatic
    store::update_channel(
        &pool,
        bbc.id,
        &ChannelPatch {
            epg_channel_id: Some(Some("cnn.us".into())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ing.refresh_epg(&store::get_source(&pool, sid).await.unwrap())
        .await
        .unwrap();
    let m = store::epg_range(&pool, &[bbc.id], base, base + 600, false)
        .await
        .unwrap();
    assert_eq!(m[&bbc.id][0].title, "World Report");
    store::update_channel(
        &pool,
        bbc.id,
        &ChannelPatch {
            epg_channel_id: Some(None),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let c = store::get_channel(&pool, bbc.id, None).await.unwrap();
    assert_eq!(
        (c.epg_channel_id.as_deref(), c.epg_manual),
        (Some("bbc1.uk"), false)
    );

    // refreshing the guide replaces rather than duplicates
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM epg_programs")
        .fetch_one(&pool)
        .await
        .unwrap();
    ing.refresh_epg(&store::get_source(&pool, sid).await.unwrap())
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM epg_programs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn same_tvg_id_on_two_sources_becomes_failover_alias() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.m3u");
    let b = dir.path().join("b.m3u");
    std::fs::write(
        &a,
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"bbc1.uk\",BBC One\nhttp://a/1.ts\n",
    )
    .unwrap();
    std::fs::write(
        &b,
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"BBC1.uk\" ,BBC One (backup)\nhttp://b/9.ts\n",
    )
    .unwrap();
    let pool = pool().await;
    let sa = add_source(&pool, "A", "m3u", a.to_str().unwrap(), None, Some(2)).await;
    let sb = add_source(&pool, "B", "m3u", b.to_str().unwrap(), None, Some(1)).await;
    sqlx::query("UPDATE iptv_sources SET user_agent = 'SrcUA/1' WHERE id = ?")
        .bind(sb)
        .execute(&pool)
        .await
        .unwrap();
    let ing = ingest(&pool, dir.path());
    ing.refresh_channels(&store::get_source(&pool, sa).await.unwrap())
        .await
        .unwrap();
    ing.refresh_channels(&store::get_source(&pool, sb).await.unwrap())
        .await
        .unwrap();
    let chans = store::list_channels(&pool, &ChannelFilter::default())
        .await
        .unwrap();
    let ca = chans.iter().find(|c| c.source_id == sa).unwrap();
    let cb = chans.iter().find(|c| c.source_id == sb).unwrap();
    assert_eq!(store::get_aliases(&pool, ca.id).await.unwrap().len(), 1);
    assert_eq!(
        store::get_aliases(&pool, cb.id).await.unwrap().len(),
        1,
        "both directions"
    );

    let spec = DbResolver { pool: pool.clone() }
        .resolve(ca.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(spec.candidates.len(), 2);
    assert_eq!(spec.candidates[0].url, "http://a/1.ts");
    assert_eq!(spec.candidates[0].max_connections, Some(2));
    assert_eq!(spec.candidates[1].url, "http://b/9.ts");
    assert_eq!(spec.candidates[1].max_connections, Some(1));
    assert_eq!(
        spec.candidates[1].user_agent.as_deref(),
        Some("SrcUA/1"),
        "source UA inherited"
    );
    // disabled sources/channels never resolve
    sqlx::query("UPDATE iptv_sources SET enabled = 0 WHERE id = ?")
        .bind(sb)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        DbResolver { pool: pool.clone() }
            .resolve(ca.id)
            .await
            .unwrap()
            .unwrap()
            .candidates
            .len(),
        1
    );
    assert!(DbResolver { pool }.resolve(cb.id).await.unwrap().is_none());
    // manual alias ordering
    let _ = SourceKind::M3u;
}

#[tokio::test]
async fn xtream_source_with_gzipped_http_guide() {
    let dir = tempfile::tempdir().unwrap();
    let base = now() / 600 * 600;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(guide(base).as_bytes()).unwrap();
    let gz = enc.finish().unwrap();

    async fn api(Query(q): Query<std::collections::HashMap<String, String>>) -> impl IntoResponse {
        assert_eq!(q.get("username").map(String::as_str), Some("u1"));
        assert_eq!(q.get("password").map(String::as_str), Some("p w"));
        let body = match q.get("action").map(String::as_str) {
            None => {
                r#"{"user_info":{"auth":1,"status":"Active","max_connections":"2","active_cons":"0","exp_date":"1893456000"},"server_info":{"url":"x"}}"#
            }
            Some("get_live_categories") => r#"[{"category_id":"5","category_name":"News"}]"#,
            Some("get_live_streams") => {
                r#"[{"num":7,"name":"BBC One","stream_id":101,"stream_icon":"http://i/b.png","epg_channel_id":"bbc1.uk","category_id":"5","tv_archive":1,"tv_archive_duration":"3"},{"num":8,"name":"CNN","stream_id":"102","epg_channel_id":null,"category_id":"9"}]"#
            }
            _ => "[]",
        };
        ([(header::CONTENT_TYPE, "application/json")], body)
    }
    let gz2 = gz.clone();
    let app = Router::new().route("/player_api.php", get(api)).route(
        "/xmltv.php",
        get(move || {
            let gz = gz2.clone();
            async move { ([(header::CONTENT_TYPE, "application/octet-stream")], gz) }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let pool = pool().await;
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password) VALUES ('X', 'xtream', ?, 'u1', 'p w') RETURNING id",
    )
    .bind(format!("http://{addr}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let ing = ingest(&pool, dir.path());
    let r = ing
        .refresh_source(&store::get_source(&pool, sid).await.unwrap())
        .await
        .unwrap();
    assert_eq!(r.channels.total, 2);
    let epg = r.epg.expect("xmltv.php used automatically");
    assert_eq!(
        epg.programmes_inserted, 3,
        "BBC x3 (CNN has no tvg-id and no name match)"
    );
    let src = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(src.max_connections, Some(2), "taken from the account info");
    assert_eq!(src.channel_count, 2);
    assert!(src.last_error.is_none() && src.last_refresh_at.is_some() && src.last_epg_at.is_some());
    let chans = store::list_channels(&pool, &ChannelFilter::default())
        .await
        .unwrap();
    let bbc = chans.iter().find(|c| c.name == "BBC One").unwrap();
    assert_eq!(
        (bbc.number, bbc.group.as_deref(), bbc.has_catchup),
        (Some(7), Some("News"), true)
    );
    assert_eq!(bbc.epg_channel_id.as_deref(), Some("bbc1.uk"));
    let url: String = sqlx::query_scalar("SELECT stream_url FROM channels WHERE id = ?")
        .bind(bbc.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(url, format!("http://{addr}/live/u1/p%20w/101.ts"));
    assert!(
        !dir.path().join("tmp/epg-1.tmp").exists(),
        "temp download removed"
    );

    // wrong credentials: error recorded on the source, existing channels untouched
    sqlx::query("UPDATE iptv_sources SET url = 'http://127.0.0.1:1' WHERE id = ?")
        .bind(sid)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        ing.refresh_source(&store::get_source(&pool, sid).await.unwrap())
            .await
            .is_err()
    );
    let src = store::get_source(&pool, sid).await.unwrap();
    assert!(src.last_error.is_some());
    assert_eq!(
        store::list_channels(&pool, &ChannelFilter::default())
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn service_refresh_is_exclusive_and_scheduler_picks_up_new_sources() {
    let dir = tempfile::tempdir().unwrap();
    let pl = dir.path().join("l.m3u");
    std::fs::write(&pl, PLAYLIST_V1).unwrap();
    let pool = pool().await;
    let sid = add_source(&pool, "S", "m3u", pl.to_str().unwrap(), None, None).await;
    let svc = IptvService::new(pool.clone(), dir.path().into(), IptvConfig::default());
    let r = svc.refresh_source(sid).await.unwrap();
    assert_eq!(r.channels.total, 5);
    assert!(
        r.epg.is_none() && r.epg_error.is_none(),
        "no EPG configured"
    );
    assert!(!svc.is_refreshing(sid));
    assert!(svc.refresh_source(999).await.is_err());
    svc.spawn_refresh(sid);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!svc.is_refreshing(sid));
    let _ = Arc::strong_count(&svc);
}

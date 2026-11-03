use reel_iptv::{ingest::Ingest, store};
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
        allow_private: true,
        gates: reel_iptv::gate::Gates::new(Default::default()),
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

fn prog(ch: &str, base: i64, title: &str) -> String {
    format!(
        "<programme start=\"{}\" stop=\"{}\" channel=\"{ch}\"><title>{title}</title></programme>",
        xt(base - 600),
        xt(base + 3000)
    )
}

fn xml(base: i64, chans: &[(&str, &str, &str)]) -> String {
    // (id, display-name, programme title)
    let mut s = String::from("<?xml version=\"1.0\"?><tv>");
    for (id, name, _) in chans {
        s.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{name}</display-name></channel>"
        ));
    }
    for (id, _, title) in chans {
        s.push_str(&prog(id, base, title));
    }
    s.push_str("</tv>");
    s
}

const PLAYLIST: &str = "#EXTM3U\n\
#EXTINF:-1 group-title=\"AR\",AR: MBC 2 HD\nhttp://prov/1.ts\n\
#EXTINF:-1 group-title=\"AR\",beIN SPORTS 1 HD \u{1d3f}\u{1d2c}\u{1d42}\nhttp://prov/2.ts\n\
#EXTINF:-1 group-title=\"UK\",UK| SKY SPORTS MAIN EVENT FHD\nhttp://prov/3.ts\n\
#EXTINF:-1 group-title=\"UK\",Channel 4 +1\nhttp://prov/4.ts\n\
#EXTINF:-1 group-title=\"UK\",Channel 4 HD\nhttp://prov/5.ts\n\
#EXTINF:-1 tvg-id=\"CNN.us\" group-title=\"US\",CNN\nhttp://prov/6.ts\n\
#EXTINF:-1 group-title=\"X\",Nothing Matches This\nhttp://prov/7.ts\n";

async fn epg_of(pool: &SqlitePool, name: &str) -> (Option<String>, Vec<String>) {
    let id: Option<String> =
        sqlx::query_scalar("SELECT epg_channel_id FROM channels WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap();
    let titles = match &id {
        Some(i) => sqlx::query_scalar(
            "SELECT title FROM epg_programs WHERE channel_tvg_id = ? ORDER BY title",
        )
        .bind(i)
        .fetch_all(pool)
        .await
        .unwrap(),
        None => vec![],
    };
    (id, titles)
}

#[tokio::test]
async fn several_guides_import_in_order_with_precedence_stats_and_name_matching() {
    let dir = tempfile::tempdir().unwrap();
    let base = now() / 600 * 600;
    let pl = dir.path().join("list.m3u");
    std::fs::write(&pl, PLAYLIST).unwrap();
    // AE1-style guide
    let a = dir.path().join("ae1.xml");
    std::fs::write(
        &a,
        xml(
            base,
            &[
                ("MBC.2.ae", "MBC 2", "A-MBC2"),
                ("beIN.Sports.1.HD.qa", "beIN Sports 1 HD", "A-bein"),
                ("CNN.us", "CNN", "A-CNN"),
                ("Unrelated.ae", "Unrelated", "A-none"),
            ],
        ),
    )
    .unwrap();
    // UK1-style guide: also carries MBC 2 (other id) and CNN.us (same id) → loses to the earlier guide
    let b = dir.path().join("uk1.xml");
    std::fs::write(
        &b,
        xml(
            base,
            &[
                ("Sky.Sports.Main.Event.uk", "Sky Sports Main Event", "B-sky"),
                ("Channel.4.uk", "Channel 4", "B-c4"),
                ("Channel.4.+1.uk", "Channel 4 +1", "B-c4plus1"),
                ("MBC.2.uk", "MBC 2", "B-MBC2"),
                ("CNN.us", "CNN", "B-CNN"),
            ],
        ),
    )
    .unwrap();
    let bad = dir.path().join("missing.xml"); // never created
    let url = |p: &std::path::Path| format!("file://{}", p.display());

    let pool = pool().await;
    let sid = add_source(&pool, "Prov", "m3u", pl.to_str().unwrap(), None, None).await;
    let set_urls = |urls: Vec<String>| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE iptv_sources SET epg_urls = ? WHERE id = ?")
                .bind(serde_json::to_string(&urls).unwrap())
                .bind(sid)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    set_urls(vec![url(&a), url(&b), url(&bad)]).await;
    let ing = ingest(&pool, dir.path());
    let src = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(src.epg_urls.len(), 3);
    let r = ing.refresh_source(&src).await.unwrap();
    assert!(
        r.epg_error.is_none(),
        "one bad guide does not fail the refresh: {:?}",
        r.epg_error
    );

    // name matching: region prefix / quality / superscripts / timeshift
    assert_eq!(
        epg_of(&pool, "AR: MBC 2 HD").await,
        (Some("MBC.2.ae".into()), vec!["A-MBC2".into()]),
        "earlier guide wins"
    );
    assert_eq!(
        epg_of(&pool, "beIN SPORTS 1 HD \u{1d3f}\u{1d2c}\u{1d42}")
            .await
            .0
            .as_deref(),
        Some("beIN.Sports.1.HD.qa")
    );
    assert_eq!(
        epg_of(&pool, "UK| SKY SPORTS MAIN EVENT FHD").await,
        (
            Some("Sky.Sports.Main.Event.uk".into()),
            vec!["B-sky".into()]
        )
    );
    assert_eq!(
        epg_of(&pool, "Channel 4 +1").await,
        (Some("Channel.4.+1.uk".into()), vec!["B-c4plus1".into()])
    );
    assert_eq!(
        epg_of(&pool, "Channel 4 HD").await,
        (Some("Channel.4.uk".into()), vec!["B-c4".into()])
    );
    // exact id: both guides carry CNN.us, the earlier one supplies the programmes
    assert_eq!(
        epg_of(&pool, "CNN").await,
        (Some("CNN.us".into()), vec!["A-CNN".into()])
    );
    assert_eq!(epg_of(&pool, "Nothing Matches This").await.0, None);
    let stale: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM epg_programs WHERE title IN ('B-MBC2', 'B-CNN', 'A-none')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stale, 0,
        "losing guides' duplicates and unwatched channels are not stored"
    );

    // per-guide stats
    let st = store::guide_stats(&pool, sid).await.unwrap();
    assert_eq!(st.len(), 3);
    assert!(st.iter().all(|g| !g.provider));
    assert_eq!(
        (st[0].channels, st[0].programmes, st[0].matched),
        (4, 3, 3),
        "{:?}",
        st[0]
    );
    assert!(st[0].last_error.is_none() && st[0].last_at.is_some());
    assert_eq!(
        (st[1].channels, st[1].programmes, st[1].matched),
        (4, 3, 3),
        "CNN.us claimed by guide 1: {:?}",
        st[1]
    );
    assert_eq!((st[2].channels, st[2].programmes), (0, 0));
    assert!(
        st[2].last_error.as_deref().unwrap_or("").len() > 3,
        "{:?}",
        st[2]
    );
    let src = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(src.epg_matched_channels, 6);
    assert_eq!(src.epg_mode, "xmltv");

    // reorder: the second guide now has precedence for MBC 2 and CNN
    set_urls(vec![url(&b), url(&a)]).await;
    let src = store::get_source(&pool, sid).await.unwrap();
    ing.refresh_source(&src).await.unwrap();
    assert_eq!(
        epg_of(&pool, "AR: MBC 2 HD").await,
        (Some("MBC.2.uk".into()), vec!["B-MBC2".into()])
    );
    assert_eq!(
        epg_of(&pool, "CNN").await,
        (Some("CNN.us".into()), vec!["B-CNN".into()])
    );
    assert_eq!(
        epg_of(&pool, "beIN SPORTS 1 HD \u{1d3f}\u{1d2c}\u{1d42}")
            .await
            .0
            .as_deref(),
        Some("beIN.Sports.1.HD.qa"),
        "only guide A has it"
    );
    let st = store::guide_stats(&pool, sid).await.unwrap();
    assert_eq!(st.len(), 2, "removed URL's row is gone");
    assert!(st[0].url.ends_with("uk1.xml") && st[1].url.ends_with("ae1.xml"));
    assert!(st.iter().all(|g| g.last_error.is_none()));
}

#[tokio::test]
async fn legacy_single_epg_url_is_the_first_entry_and_every_guide_failing_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let base = now() / 600 * 600;
    let pl = dir.path().join("list.m3u");
    std::fs::write(&pl, PLAYLIST).unwrap();
    let a = dir.path().join("ae1.xml");
    std::fs::write(&a, xml(base, &[("MBC.2.ae", "MBC 2", "A-MBC2")])).unwrap();
    let pool = pool().await;
    let sid = add_source(
        &pool,
        "Prov",
        "m3u",
        pl.to_str().unwrap(),
        Some(&format!("file://{}", a.display())),
        None,
    )
    .await;
    let src = store::get_source(&pool, sid).await.unwrap();
    assert_eq!(
        src.epg_urls,
        vec![format!("file://{}", a.display())],
        "legacy column is read as a one-entry list"
    );
    let ing = ingest(&pool, dir.path());
    let r = ing.refresh_source(&src).await.unwrap();
    assert!(r.epg.is_some());
    assert_eq!(
        epg_of(&pool, "AR: MBC 2 HD").await.1,
        vec!["A-MBC2".to_string()]
    );

    // all guides failing → the refresh reports it
    sqlx::query("UPDATE iptv_sources SET epg_urls = ? WHERE id = ?")
        .bind(
            serde_json::to_string(&[format!("file://{}/nope.xml", dir.path().display())]).unwrap(),
        )
        .bind(sid)
        .execute(&pool)
        .await
        .unwrap();
    let src = store::get_source(&pool, sid).await.unwrap();
    assert!(ing.refresh_epg(&src).await.is_err());
    let st = store::guide_stats(&pool, sid).await.unwrap();
    assert!(st[0].last_error.is_some());
}

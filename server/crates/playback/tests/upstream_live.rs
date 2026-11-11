//! Live-HLS behaviour of the loopback upstream proxy (used by the IPTV channel hub): referer and
//! user agent on every request, tokens reused across playlist reloads, sliding expiry, reloads from
//! the redirect target, and a lease that revokes everything derived from it.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use reel_playback::upstream::{UpstreamOpts, UpstreamProxy};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Prov {
    /// (path, user agent, referer) of every request.
    log: Mutex<Vec<(String, String, String)>>,
    reloads: AtomicUsize,
}

fn note(p: &Prov, path: &str, h: &HeaderMap) {
    let get = |k: header::HeaderName| {
        h.get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    p.log
        .lock()
        .unwrap()
        .push((path.into(), get(header::USER_AGENT), get(header::REFERER)));
}

async fn start() -> (Arc<Prov>, u16) {
    let p = Arc::new(Prov::default());
    let app = Router::new()
        // the provider's public URL carries the login and redirects to a load balancer
        .route(
            "/live/bob/s3cret/1.m3u8",
            get(|State(p): State<Arc<Prov>>, h: HeaderMap| async move {
                note(&p, "/live/bob/s3cret/1.m3u8", &h);
                Redirect::temporary("/lb/sess42/1.m3u8")
            }),
        )
        // a sliding live window: each reload drops the oldest segment and adds a new one
        .route(
            "/lb/sess42/1.m3u8",
            get(|State(p): State<Arc<Prov>>, h: HeaderMap| async move {
                note(&p, "/lb/sess42/1.m3u8", &h);
                let n = p.reloads.fetch_add(1, Ordering::SeqCst);
                let mut pl = format!(
                    "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{n}\n\
                     #EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n"
                );
                for i in n..n + 3 {
                    pl.push_str(&format!("#EXTINF:2,\nseg{i}.ts\n"));
                }
                (
                    [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                    pl,
                )
            }),
        )
        .route(
            "/lb/sess42/{file}",
            get(
                |State(p): State<Arc<Prov>>, Path(f): Path<String>, h: HeaderMap| async move {
                    note(&p, &format!("/lb/sess42/{f}"), &h);
                    let r: Response = format!("DATA-{f}").into_response();
                    r
                },
            ),
        )
        .with_state(p.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (p, port)
}

fn proxy() -> UpstreamProxy {
    // like `reel_iptv::net::client`: no automatic `Referer` (the login URL) on redirects
    let client = reqwest::Client::builder().referer(false).build().unwrap();
    UpstreamProxy::new(client, Arc::new(|_| Ok(())))
}

async fn get_text(url: &str) -> (u16, String) {
    let r = reqwest::get(url).await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// Every loopback URI of a playlist: (segment lines, `URI="…"` attributes).
fn uris(pl: &str) -> (Vec<String>, Vec<String>) {
    let segs = pl
        .lines()
        .filter(|l| l.starts_with("http://"))
        .map(str::to_string)
        .collect();
    let attrs = pl
        .lines()
        .filter_map(|l| l.split_once("URI=\""))
        .map(|(_, r)| r.split('"').next().unwrap().to_string())
        .collect();
    (segs, attrs)
}

fn live_opts(idle: Duration) -> UpstreamOpts {
    UpstreamOpts {
        user_agent: "LiveUA/1".into(),
        referer: Some("https://portal.example/".into()),
        idle: Some(idle),
    }
}

#[tokio::test]
async fn live_lease_reuses_tokens_reloads_from_the_redirect_and_forwards_headers() {
    let (prov, port) = start().await;
    let proxy = proxy();
    let root = format!("http://127.0.0.1:{port}/live/bob/s3cret/1.m3u8");
    let idle = Duration::from_secs(60);
    let lease = proxy.lease_with(&root, &live_opts(idle), idle).unwrap();
    assert!(!lease.url().contains("s3cret") && !lease.url().contains(&port.to_string()));

    let (s, pl1) = get_text(lease.url()).await;
    assert_eq!(s, 200);
    assert!(!pl1.contains("sess42") && !pl1.contains("s3cret"), "{pl1}");
    let (segs1, keys1) = uris(&pl1);
    assert_eq!((segs1.len(), keys1.len()), (3, 1), "{pl1}");
    let after_first = proxy.token_count();
    assert_eq!(after_first, 5, "root + key + 3 segments");

    // reloads: the overlapping segments and the key keep their tokens, only new ones are added
    let (_, pl2) = get_text(lease.url()).await;
    let (_, pl3) = get_text(lease.url()).await;
    let (segs2, keys2) = uris(&pl2);
    let (segs3, keys3) = uris(&pl3);
    assert_eq!(keys1, keys2);
    assert_eq!(keys2, keys3);
    assert_eq!(segs1[1..], segs2[..2]);
    assert_eq!(segs2[1..], segs3[..2]);
    assert_eq!(proxy.token_count(), after_first + 2);

    let (s, body) = get_text(&segs3[2]).await;
    assert_eq!((s, body.as_str()), (200, "DATA-seg4.ts"));
    assert_eq!(get_text(&keys3[0]).await.1, "DATA-key.bin");

    {
        let log = prov.log.lock().unwrap();
        // the login URL was requested once; reloads went to where it redirected
        let logins = log.iter().filter(|(p, _, _)| p.contains("s3cret")).count();
        assert_eq!(logins, 1, "{log:?}");
        assert!(log.len() >= 6, "{log:?}");
        for (p, ua, referer) in log.iter() {
            assert_eq!(ua, "LiveUA/1", "{p}");
            assert_eq!(referer, "https://portal.example/", "{p}");
        }
    }

    // dropping the lease revokes the root and every derived token at once
    drop(lease);
    assert_eq!(proxy.token_count(), 0);
    assert_eq!(get_text(&segs3[2]).await.0, 404);
    assert_eq!(get_text(&keys1[0]).await.0, 404);
}

#[tokio::test]
async fn live_tokens_expire_after_idle_but_stay_alive_while_used() {
    let (_prov, port) = start().await;
    let proxy = proxy();
    let idle = Duration::from_millis(700);
    let lease = proxy
        .lease_with(
            &format!("http://127.0.0.1:{port}/lb/sess42/1.m3u8"),
            &live_opts(idle),
            idle,
        )
        .unwrap();
    // reloaded well past the idle time: still alive, and so are the segments it lists
    let mut last = String::new();
    for _ in 0..4 {
        let (s, pl) = get_text(lease.url()).await;
        assert_eq!(s, 200);
        last = pl;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let (segs, _) = uris(&last);
    assert_eq!(get_text(&segs[2]).await.0, 200);
    // unused for longer than `idle`: gone, even though the lease still exists
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(get_text(lease.url()).await.0, 404);
    assert_eq!(get_text(&segs[2]).await.0, 404);
}

#[tokio::test]
async fn fixed_expiry_leases_keep_their_url_and_have_no_referer() {
    let (prov, port) = start().await;
    let proxy = proxy();
    let lease = proxy
        .lease(
            &format!("http://127.0.0.1:{port}/live/bob/s3cret/1.m3u8"),
            "VodUA",
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(get_text(lease.url()).await.0, 200);
    assert_eq!(get_text(lease.url()).await.0, 200);
    let log = prov.log.lock().unwrap();
    // without `idle` the registered URL is requested every time (VOD behaviour unchanged)
    assert_eq!(
        log.iter().filter(|(p, _, _)| p.contains("s3cret")).count(),
        2
    );
    assert!(log.iter().all(|(_, ua, r)| ua == "VodUA" && r.is_empty()));
}

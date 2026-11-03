mod common;

use std::{
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode, header},
    response::IntoResponse,
    routing::get as route_get,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use serde_json::{Value, json};
use tower::ServiceExt;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-encoders")
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("libx264") && s.contains(" aac ")
        })
        .unwrap_or(false)
}

fn make_ts(path: &std::path::Path, secs: u32) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=128x96:rate=25:duration={secs}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=duration={secs}"))
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-f",
            "mpegts",
        ])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

/// A tiny IPTV provider: playlist, XMLTV guide, logo and looping MPEG-TS channels.
struct Provider {
    addr: std::net::SocketAddr,
}

struct PState {
    ts: Bytes,
    step: usize,
    hits: Mutex<Vec<String>>,
}

fn xt(ts: i64) -> String {
    // civil_from_days (Hinnant)
    let (days, secs) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

impl Provider {
    async fn start(ts_file: &std::path::Path, secs: u32) -> (Provider, Arc<PState>) {
        let ts = Bytes::from(std::fs::read(ts_file).unwrap());
        let step = ((ts.len() as f64 / f64::from(secs) * 2.0 / 50.0) as usize).max(1) | 1;
        let st = Arc::new(PState {
            ts,
            step,
            hits: Mutex::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/list.m3u",
                route_get(move || async move {
                    format!(
                        "#EXTM3U\n#EXTINF:-1 tvg-id=\"news.uk\" tvg-logo=\"http://{addr}/logo.png\" group-title=\"News\" tvg-chno=\"1\",UK News HD\nhttp://{addr}/live/news\n\
                         #EXTINF:-1 tvg-id=\"sport.uk\" tvg-logo=\"http://{addr}/notanimage.png\" group-title=\"Sports\" tvg-chno=\"2\",UK Sport\nhttp://{addr}/live/sport\n\
                         #EXTINF:-1 group-title=\"Movies\",Cinema\nhttp://{addr}/live/cinema\n"
                    )
                }),
            )
            .route(
                "/guide.xml",
                route_get(|| async {
                    let base = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64
                        / 600
                        * 600;
                    let p = |ch: &str, a: i64, b: i64, t: &str| {
                        format!(
                            "<programme start=\"{}\" stop=\"{}\" channel=\"{ch}\"><title>{t}</title><desc>About {t}</desc></programme>",
                            xt(base + a),
                            xt(base + b)
                        )
                    };
                    format!(
                        "<tv><channel id=\"news.uk\"><display-name>UK News</display-name></channel>\
                         <channel id=\"sport.uk\"><display-name>UK Sport</display-name></channel>{}{}{}</tv>",
                        p("news.uk", -900, 900, "Headlines"),
                        p("news.uk", 900, 3600, "Weather"),
                        p("sport.uk", -300, 1800, "Match of the Day"),
                    )
                }),
            )
            .route(
                "/logo.png",
                route_get(|| async {
                    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
                    png.extend_from_slice(&[0u8; 64]);
                    ([(header::CONTENT_TYPE, "image/png")], png)
                }),
            )
            .route(
                "/notanimage.png",
                route_get(|| async { ([(header::CONTENT_TYPE, "image/png")], "<html>nope</html>") }),
            )
            .route(
                "/live/{name}",
                route_get(
                    |State(st): State<Arc<PState>>, axum::extract::Path(name): axum::extract::Path<String>| async move {
                        st.hits.lock().unwrap().push(name);
                        let (data, step) = (st.ts.clone(), st.step);
                        let stream = futures_util::stream::unfold(
                            (0usize, tokio::time::interval(Duration::from_millis(20))),
                            move |(pos, mut tick)| {
                                let data = data.clone();
                                async move {
                                    tick.tick().await;
                                    let n = step.min(data.len() - pos);
                                    Some((
                                        Ok::<_, std::io::Error>(data.slice(pos..pos + n)),
                                        ((pos + n) % data.len(), tick),
                                    ))
                                }
                            },
                        );
                        Response::builder()
                            .header(header::CONTENT_TYPE, "video/mp2t")
                            .body(Body::from_stream(stream))
                            .unwrap()
                            .into_response()
                    },
                ),
            )
            .with_state(st.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (Provider { addr }, st)
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }
}

struct Env {
    state: AppState,
    raw: Router,
    admin: Router,
    _dir: tempfile::TempDir,
}

async fn env(cfg: impl FnOnce(&mut reel_core::Config)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let mut c = reel_core::Config {
        data_dir: dir.path().join("data"),
        iptv_grace_secs: 1,
        iptv_stall_secs: 3,
        ..Default::default()
    };
    cfg(&mut c);
    let state = AppState::new(db, c);
    state.iptv.hub().spawn_reaper();
    state.iptv.live.spawn_reaper();
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    Env {
        state,
        raw,
        admin,
        _dir: dir,
    }
}

fn send(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    let app = app.clone();
    let mut r = Request::builder().method(m).uri(uri);
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let req = r.body(b).unwrap();
    async move {
        let res = app.oneshot(req).await.unwrap();
        let s = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

fn get(app: &Router, uri: &str) -> impl std::future::Future<Output = (StatusCode, Value)> + use<> {
    send(app, "GET", uri, None)
}

/// Add the provider as an M3U source (via the admin API) and wait for the first refresh.
async fn add_source(e: &Env, p: &Provider, max: Option<u32>) -> i64 {
    let (s, v) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/iptv/sources",
        Some(
            json!({"name":"Prov","kind":"m3u","url":format!("{}/list.m3u", p.base()),
                    "epg_url":format!("{}/guide.xml", p.base()),"max_connections":max}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    for _ in 0..100 {
        let (_, s) = get(&e.admin, &format!("/api/v1/admin/iptv/sources/{id}")).await;
        if s["last_epg_at"].is_i64() {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("source never refreshed");
}

async fn channel_id(e: &Env, name: &str) -> i64 {
    let (_, l) = get(&e.admin, "/api/v1/iptv/channels").await;
    l.as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

async fn first_frame(res: Response<Body>) -> Bytes {
    let mut body = res.into_body().into_data_stream();
    use futures_util::StreamExt;
    tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .expect("first bytes within 5 s")
        .expect("stream not empty")
        .unwrap()
}

async fn bob(e: &Env) -> Router {
    let (s, inv) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/invites",
        // invited accounts start without feature access: this one watches live TV
        Some(json!({"limits": {}, "permissions": {"iptv_access": true, "vod_access": true, "can_request": false, "can_cast": false}})),
    )
    .await;
    assert_eq!(s, 201);
    let code = inv["code"].as_str().unwrap();
    let (s, v) = send(
        &e.raw,
        "POST",
        &format!("/api/v1/auth/invite/{code}/accept"),
        Some(json!({"username":"bob","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    common::with_token(e.raw.clone(), v["token"].as_str().unwrap())
}

#[tokio::test]
async fn ingest_channels_epg_favorites_and_signed_logos() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 4);
    let (prov, _) = Provider::start(&ts, 4).await;
    // the test provider listens on loopback, so allow private logo fetches here
    let e = env(|c| c.allow_private_logo_fetch = true).await;
    // unauthenticated
    assert_eq!(get(&e.raw, "/api/v1/iptv/channels").await.0, 401);
    let id = add_source(&e, &prov, None).await;

    let (_, chans) = get(&e.admin, "/api/v1/iptv/channels").await;
    let names: Vec<&str> = chans
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["UK News HD", "UK Sport", "Cinema"],
        "ordered by channel number, then name"
    );
    assert_eq!(chans[0]["number"], 1);
    assert_eq!(chans[0]["group"], "News");
    assert_eq!(chans[0]["has_epg"], true);
    assert_eq!(chans[2]["has_epg"], false);
    assert!(chans[2].get("logo_url").is_none_or(Value::is_null));
    let (_, groups) = get(&e.admin, "/api/v1/iptv/groups").await;
    assert_eq!(groups.as_array().unwrap().len(), 3);
    assert_eq!(
        get(&e.admin, "/api/v1/iptv/channels?group=Sports")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        get(&e.admin, "/api/v1/iptv/channels?q=cine")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // source: password never exposed; fuzzy EPG map for "UK News HD" ↔ "UK News"
    let (_, s) = get(&e.admin, &format!("/api/v1/admin/iptv/sources/{id}")).await;
    assert_eq!(s["channel_count"], 3);
    assert_eq!(s["password_set"], false);
    assert!(s["last_error"].is_null());

    // EPG grid + now/next
    let news = channel_id(&e, "UK News HD").await;
    let sport = channel_id(&e, "UK Sport").await;
    let cinema = channel_id(&e, "Cinema").await;
    let (s, grid) = get(
        &e.admin,
        &format!("/api/v1/iptv/epg?channels={news},{sport},{cinema}"),
    )
    .await;
    assert_eq!(s, 200, "{grid}");
    assert_eq!(grid["channels"].as_array().unwrap().len(), 3);
    let progs = grid["channels"][0]["programs"].as_array().unwrap();
    assert_eq!(progs[0]["title"], "Headlines");
    assert!(progs[0].get("description").is_none(), "compact by default");
    assert_eq!(grid["channels"][2]["programs"], json!([]));
    let (_, detail) = get(
        &e.admin,
        &format!("/api/v1/iptv/epg?channels={news}&detail=true"),
    )
    .await;
    assert_eq!(
        detail["channels"][0]["programs"][0]["description"],
        "About Headlines"
    );
    assert_eq!(get(&e.admin, "/api/v1/iptv/epg?channels=x").await.0, 400);
    assert_eq!(
        get(&e.admin, "/api/v1/iptv/epg?from=100&to=99").await.0,
        400
    );
    let (_, nn) = get(
        &e.admin,
        &format!("/api/v1/iptv/now?channels={news},{sport}"),
    )
    .await;
    assert_eq!(nn[0]["now"]["title"], "Headlines");
    assert_eq!(nn[0]["next"]["title"], "Weather");
    assert_eq!(nn[1]["now"]["title"], "Match of the Day");
    assert!(nn[1]["next"].is_null());

    // favorites are per user
    let bob = bob(&e).await;
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/iptv/channels/{news}/favorite"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(&e.admin, "POST", "/api/v1/iptv/channels/999/favorite", None)
            .await
            .0,
        404
    );
    let favs = get(&e.admin, "/api/v1/iptv/channels?favorites=true")
        .await
        .1;
    assert_eq!(favs.as_array().unwrap().len(), 1);
    assert_eq!(favs[0]["favorite"], true);
    assert_eq!(
        get(&bob, "/api/v1/iptv/channels?favorites=true").await.1,
        json!([])
    );
    assert_eq!(
        get(&bob, "/api/v1/iptv/channels").await.1[0]["favorite"],
        false
    );
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/iptv/channels/{news}/favorite"),
            None
        )
        .await
        .0,
        204
    );

    // logos: signed URL works without a token; non-images and tampering are refused
    let logo = chans[0]["logo_url"].as_str().unwrap().to_string();
    assert!(
        logo.starts_with(&format!("/api/v1/iptv/logo/{news}?exp=")),
        "{logo}"
    );
    let res = e
        .raw
        .clone()
        .oneshot(Request::get(&logo).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/png");
    let bad = logo.replace("sig=", "sig=x");
    assert_eq!(
        e.raw
            .clone()
            .oneshot(Request::get(bad).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        401
    );
    let sport_logo = get(&e.admin, "/api/v1/iptv/channels?q=sport").await.1[0]["logo_url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        e.raw
            .clone()
            .oneshot(Request::get(sport_logo).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        404,
        "html is not an image"
    );
    assert!(
        e.state.config.data_dir.join("iptv/logos").exists(),
        "cached on disk"
    );
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn tune_passthrough_and_hls_use_signed_urls_without_headers() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 4);
    let (prov, st) = Provider::start(&ts, 4).await;
    let e = env(|_| {}).await;
    add_source(&e, &prov, Some(2)).await;
    let news = channel_id(&e, "UK News HD").await;

    // passthrough: raw TS, starts with the PAT, readable by a header-less native player
    let (s, t) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/iptv/channels/{news}/tune"),
        Some(json!({"mode":"passthrough"})),
    )
    .await;
    assert_eq!(s, 200, "{t}");
    assert_eq!(t["mode"], "passthrough");
    assert_eq!(t["channel_name"], "UK News HD");
    let url = t["url"].as_str().unwrap().to_string();
    let sid = t["session_id"].as_str().unwrap().to_string();
    assert!(
        url.starts_with(&format!("/api/v1/iptv/live/{sid}/stream.ts?exp=")),
        "{url}"
    );
    assert_eq!(
        e.raw
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/iptv/live/{sid}/stream.ts"))
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        401
    );
    let res = e
        .raw
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "video/mp2t");
    let first = first_frame(res).await;
    assert_eq!(first[0], 0x47);
    assert_eq!(
        ((first[1] & 0x1F) as u16) << 8 | first[2] as u16,
        0,
        "PAT first"
    );

    // admin sees the session, the hub and per-channel metrics
    let (_, hub) = get(&e.admin, "/api/v1/admin/iptv/hub").await;
    assert_eq!(hub["sessions"][0]["id"], sid);
    assert_eq!(hub["sessions"][0]["username"], "admin");
    assert_eq!(hub["sources"][0]["open_connections"], 1);
    assert_eq!(hub["sources"][0]["max_connections"], 2);
    let m = &hub["channels"][0];
    assert_eq!(m["name"], "UK News HD");
    assert!(m["ttfb_ms"].is_u64() && m["bytes_total"].as_u64().unwrap() > 0);
    assert_eq!(m["video_codec"], "h264");

    // second viewer of the same channel → still ONE provider connection
    let (s, t2) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/iptv/channels/{news}/tune"),
        Some(json!({"mode":"hls","preset":"web_chrome"})),
    )
    .await;
    assert_eq!(s, 200, "{t2}");
    assert_eq!(t2["mode"], "hls");
    assert_eq!(
        t2["hls"],
        json!({"video":"copy","audio":"copy","encoder":"copy"})
    );
    let sid2 = t2["session_id"].as_str().unwrap().to_string();
    let url2 = t2["url"].as_str().unwrap().to_string();
    assert!(url2.contains("/index.m3u8?exp="));
    assert_eq!(
        st.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| *h == "news")
            .count(),
        1,
        "one upstream for two viewers"
    );

    // HLS playlist via signature only: every URI is signed so a player can follow it
    let res = e
        .raw
        .clone()
        .oneshot(Request::get(&url2).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    let text =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4?exp="), "{text}");
    let seg = text.lines().find(|l| l.starts_with("seg_")).unwrap();
    assert!(seg.contains("&sig="));
    let res = e
        .raw
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/iptv/live/{sid2}/{seg}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "segment with propagated signature");
    assert_eq!(res.headers()[header::CONTENT_TYPE], "video/mp4");
    let no_sig = seg.split('?').next().unwrap();
    let res = e
        .raw
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/iptv/live/{sid2}/{no_sig}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // a signature for one session is not valid for another
    let q = url2.split('?').nth(1).unwrap();
    let res = e
        .raw
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/iptv/live/{sid}/stream.ts?{q}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // sessions belong to their owner
    let bob = bob(&e).await;
    assert_eq!(
        send(
            &bob,
            "DELETE",
            &format!("/api/v1/iptv/sessions/{sid}"),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(get(&bob, "/api/v1/admin/iptv/hub").await.0, 403);
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/iptv/sessions/{sid}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/iptv/sessions/{sid}"),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/iptv/sessions/{sid2}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/iptv/channels/999/tune",
            Some(json!({}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/iptv/channels/{news}/tune"),
            Some(json!({"preset":"nope"}))
        )
        .await
        .0,
        400
    );
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn provider_connection_budget_returns_429_and_evicts_warm_channels() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 4);
    let (prov, st) = Provider::start(&ts, 4).await;
    let e = env(|c| c.iptv_grace_secs = 30).await;
    add_source(&e, &prov, Some(1)).await;
    let (news, sport) = (
        channel_id(&e, "UK News HD").await,
        channel_id(&e, "UK Sport").await,
    );
    let bob = bob(&e).await;
    let tune = |app: &Router, id: i64| {
        let app = app.clone();
        async move {
            send(
                &app,
                "POST",
                &format!("/api/v1/iptv/channels/{id}/tune"),
                Some(json!({"mode":"passthrough"})),
            )
            .await
        }
    };
    let (s, t1) = tune(&e.admin, news).await;
    assert_eq!(s, 200);
    // the only provider slot is held by another user's viewer: friendly 429 for a non-admin
    let (s, v) = tune(&bob, sport).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    let msg = v["error"].as_str().unwrap();
    assert!(
        msg.contains("in use by other viewers") && !msg.contains("UK News HD"),
        "{msg}"
    );
    // the admin stops watching: the channel stays warm but is evicted for bob
    send(
        &e.admin,
        "DELETE",
        &format!(
            "/api/v1/iptv/sessions/{}",
            t1["session_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (s, v) = tune(&bob, sport).await;
    assert_eq!(s, 200, "{v}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let hits = st.hits.lock().unwrap().clone();
    assert_eq!(hits, vec!["news", "sport"]);
    let (_, hub) = get(&e.admin, "/api/v1/admin/iptv/hub").await;
    assert_eq!(hub["sources"][0]["open_connections"], 1);
    assert_eq!(
        hub["channels"].as_array().unwrap().len(),
        1,
        "news was evicted"
    );
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn user_stream_limit_counts_live_sessions() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 4);
    let (prov, _) = Provider::start(&ts, 4).await;
    let e = env(|_| {}).await;
    add_source(&e, &prov, None).await;
    let news = channel_id(&e, "UK News HD").await;
    let (s, inv) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/invites",
        Some(json!({"limits": {"max_streams": 1}, "permissions": {"iptv_access": true, "vod_access": false, "can_request": false, "can_cast": false}})),
    )
    .await;
    assert_eq!(s, 201);
    let (_, v) = send(
        &e.raw,
        "POST",
        &format!(
            "/api/v1/auth/invite/{}/accept",
            inv["code"].as_str().unwrap()
        ),
        Some(json!({"username":"limited","password":"correct horse battery"})),
    )
    .await;
    let limited = common::with_token(e.raw.clone(), v["token"].as_str().unwrap());
    let url = format!("/api/v1/iptv/channels/{news}/tune");
    let (s, t) = send(&limited, "POST", &url, Some(json!({"mode":"passthrough"}))).await;
    assert_eq!(s, 200);
    // newest wins: the user's own older live session never blocks the new tune
    let (s, t2) = send(&limited, "POST", &url, Some(json!({"mode":"passthrough"}))).await;
    assert_eq!(s, 200, "{t2}");
    assert_eq!(e.state.iptv.live.count_for_user(2), 1);
    assert!(
        e.state
            .iptv
            .live
            .session(t["session_id"].as_str().unwrap())
            .is_none()
    );
    let t = t2;
    send(
        &limited,
        "DELETE",
        &format!(
            "/api/v1/iptv/sessions/{}",
            t["session_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(
        send(&limited, "POST", &url, Some(json!({"mode":"passthrough"})))
            .await
            .0,
        200
    );
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn admin_source_and_channel_management() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 2);
    let (prov, _) = Provider::start(&ts, 2).await;
    let e = env(|_| {}).await;
    let bob = bob(&e).await;
    // validation + permissions
    let post = |b: Value| send(&e.admin, "POST", "/api/v1/admin/iptv/sources", Some(b));
    assert_eq!(
        post(json!({"name":"x","kind":"plex","url":"http://a"}))
            .await
            .0,
        400
    );
    assert_eq!(
        post(json!({"name":"x","kind":"m3u","url":"file:///etc/passwd"}))
            .await
            .0,
        400
    );
    assert_eq!(
        post(json!({"name":"x","kind":"xtream","url":"http://a"}))
            .await
            .0,
        400,
        "needs credentials"
    );
    assert_eq!(
        send(
            &bob,
            "POST",
            "/api/v1/admin/iptv/sources",
            Some(json!({"name":"x","kind":"m3u","url":"http://a"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(get(&bob, "/api/v1/admin/iptv/sources").await.0, 403);

    // test without saving
    let (s, t) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/iptv/sources/test",
        Some(json!({"name":"t","kind":"m3u","url":format!("{}/list.m3u", prov.base())})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(
        (t["ok"].clone(), t["channels"].clone()),
        (json!(true), json!(3))
    );
    let (_, t) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/iptv/sources/test",
        Some(json!({"name":"t","kind":"m3u","url":"http://127.0.0.1:1/x.m3u"})),
    )
    .await;
    assert_eq!(t["ok"], false);
    assert!(t["message"].as_str().unwrap().contains("cannot reach"));

    let id = add_source(&e, &prov, None).await;
    let (_, t) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/admin/iptv/sources/{id}/test"),
        None,
    )
    .await;
    assert_eq!(t["channels"], 3);
    // update: limit + rename; refresh → 202
    let (s, v) = send(
        &e.admin,
        "PATCH",
        &format!("/api/v1/admin/iptv/sources/{id}"),
        Some(json!({"name":"Main","max_connections":2,"user_agent":"UA/1"})),
    )
    .await;
    assert_eq!(
        (s, v["name"].clone(), v["max_connections"].clone()),
        (StatusCode::OK, json!("Main"), json!(2))
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/admin/iptv/sources/{id}/refresh"),
            None
        )
        .await
        .0,
        202
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/iptv/sources/999/refresh",
            None
        )
        .await
        .0,
        404
    );

    // channel edits: rename, number, disable, epg override, aliases
    let (_, adm) = get(
        &e.admin,
        &format!("/api/v1/admin/iptv/channels?source={id}"),
    )
    .await;
    assert_eq!(adm.as_array().unwrap().len(), 3);
    let cinema = adm
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Cinema")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let sport = adm
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "UK Sport")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let patch = |cid: i64, b: Value| {
        send(
            &e.admin,
            "PATCH",
            &format!("/api/v1/admin/iptv/channels/{cid}"),
            Some(b),
        )
    };
    assert_eq!(
        patch(
            cinema,
            json!({"name":"Cinema HD","number":50,"epg_channel_id":"sport.uk"})
        )
        .await
        .0,
        204
    );
    let (_, l) = get(&e.admin, "/api/v1/iptv/channels?q=cinema").await;
    assert_eq!(
        (
            l[0]["name"].clone(),
            l[0]["number"].clone(),
            l[0]["has_epg"].clone()
        ),
        (json!("Cinema HD"), json!(50), json!(true))
    );
    let (_, nn) = get(&e.admin, &format!("/api/v1/iptv/now?channels={cinema}")).await;
    assert_eq!(
        nn[0]["now"]["title"], "Match of the Day",
        "manual EPG mapping"
    );
    assert_eq!(
        patch(cinema, json!({"epg_channel_id": null, "number": null}))
            .await
            .0,
        204
    );
    assert_eq!(patch(sport, json!({"enabled": false})).await.0, 204);
    assert_eq!(
        get(&e.admin, "/api/v1/iptv/channels")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        2,
        "disabled channel hidden"
    );
    assert_eq!(
        get(&bob, "/api/v1/iptv/channels")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(patch(999, json!({"enabled": true})).await.0, 404);
    assert_eq!(patch(cinema, json!({"name": "  "})).await.0, 400);
    let put = |b: Value| {
        send(
            &e.admin,
            "PUT",
            &format!("/api/v1/admin/iptv/channels/{cinema}/aliases"),
            Some(b),
        )
    };
    assert_eq!(
        put(json!([{"channel_id": sport, "priority": 1}])).await.0,
        204
    );
    assert_eq!(
        put(json!([{"channel_id": cinema, "priority": 1}])).await.0,
        400,
        "no self alias"
    );
    assert_eq!(
        put(json!([{"channel_id": 12345, "priority": 1}])).await.0,
        400
    );
    let (_, adm) = get(
        &e.admin,
        &format!("/api/v1/admin/iptv/channels?source={id}"),
    )
    .await;
    let c = adm
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == cinema)
        .unwrap();
    assert_eq!(c["aliases"], json!([{"channel_id": sport, "priority": 1}]));

    // audit trail + delete cascades
    let (_, audit) = get(&e.admin, "/api/v1/admin/audit").await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    for a in [
        "iptv_source_create",
        "iptv_source_update",
        "iptv_source_refresh",
        "iptv_channel_update",
        "iptv_aliases_set",
    ] {
        assert!(actions.contains(&a), "{a} in {actions:?}");
    }
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/admin/iptv/sources/{id}"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(get(&e.admin, "/api/v1/iptv/channels").await.1, json!([]));
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/admin/iptv/sources/{id}"),
            None
        )
        .await
        .0,
        404
    );
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn ssrf_guard_defaults_and_strict_mode() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 2);
    let (prov, _) = Provider::start(&ts, 2).await;

    // defaults: playlists/EPG/streams on the LAN (here: loopback) work, third-party logos do not
    let e = env(|_| {}).await;
    add_source(&e, &prov, None).await;
    let (_, chans) = get(&e.admin, "/api/v1/iptv/channels").await;
    let logo = chans[0]["logo_url"].as_str().unwrap().to_string();
    let res = e
        .raw
        .clone()
        .oneshot(Request::get(&logo).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "loopback logo host refused by default");
    assert!(!e.state.config.data_dir.join("iptv/logos").exists());
    e.state.iptv.shutdown().await;

    // strict: private sources are refused up front, with a pointer to the setting
    let e = env(|c| c.allow_private_fetch = false).await;
    for url in [
        format!("{}/list.m3u", prov.base()),
        "http://169.254.169.254/latest/meta-data".to_string(),
        "http://localhost:8080/list.m3u".to_string(),
    ] {
        let (s, v) = send(
            &e.admin,
            "POST",
            "/api/v1/admin/iptv/sources",
            Some(json!({"name":"x","kind":"m3u","url":url})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{url}: {v}");
        assert!(
            v["error"].as_str().unwrap().contains("allow_private_fetch"),
            "{v}"
        );
    }
    let (s, _) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/iptv/sources/test",
        Some(json!({"name":"x","kind":"m3u","url":format!("{}/list.m3u", prov.base())})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn source_dto_masks_guide_credentials_exposes_epg_stats_and_can_rerun_only_the_guide() {
    let e = env(|_| {}).await;
    let dir = tempfile::tempdir().unwrap();
    let guide = dir.path().join("guide.xml");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let stamp = |t: i64| {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
            secs / 3600,
            secs % 3600 / 60,
            secs % 60
        )
    };
    std::fs::write(
        &guide,
        format!(
            "<?xml version=\"1.0\"?><tv><channel id=\"c1\"><display-name>One</display-name></channel>\
             <programme start=\"{}\" stop=\"{}\" channel=\"c1\"><title>Now</title></programme></tv>",
            stamp(now - 600),
            stamp(now + 3000)
        ),
    )
    .unwrap();
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO iptv_sources (name, kind, url, username, password, epg_url, detected_epg_url, last_error) \
         VALUES ('X','xtream','http://p.example','bob','hunter2', ?1, \
         'http://p.example/xmltv.php?username=bob&password=hunter2', \
         'EPG: error sending request for url (http://p.example/xmltv.php?username=bob&password=hunter2)') RETURNING id",
    )
    .bind(format!("file://{}", guide.display()))
    .fetch_one(&e.state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO channels (source_id, ext_id, name, stream_url, tvg_id) VALUES (?, '1', 'One', 'http://p.example/live/bob/hunter2/1.ts', 'c1')")
        .bind(sid)
        .execute(&e.state.db)
        .await
        .unwrap();

    let (s, v) = get(&e.admin, &format!("/api/v1/admin/iptv/sources/{sid}")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["detected_epg_url"],
        "http://p.example/xmltv.php?username=***&password=***"
    );
    let raw = v.to_string();
    assert!(
        !raw.contains("hunter2") || raw.contains("password_set"),
        "{raw}"
    );
    assert!(
        !v["last_error"].as_str().unwrap().contains("hunter2"),
        "{v}"
    );
    assert!(v["last_error"].as_str().unwrap().contains("password=***"));
    assert_eq!(v["password_set"], true);
    for k in [
        "epg_channel_count",
        "epg_programme_count",
        "epg_matched_channels",
    ] {
        assert_eq!(v[k], 0, "{k}");
    }
    let list = get(&e.admin, "/api/v1/admin/iptv/sources")
        .await
        .1
        .to_string();
    assert!(!list.contains("hunter2"), "{list}");

    // re-run only the guide import
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/iptv/sources/999/refresh-epg",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &e.raw,
            "POST",
            &format!("/api/v1/admin/iptv/sources/{sid}/refresh-epg"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/admin/iptv/sources/{sid}/refresh-epg"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let mut v = Value::Null;
    for _ in 0..100 {
        v = get(&e.admin, &format!("/api/v1/admin/iptv/sources/{sid}"))
            .await
            .1;
        if v["last_epg_at"].is_i64() && v["refreshing"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(v["last_epg_at"].is_i64(), "{v}");
    assert_eq!(
        (
            v["epg_channel_count"].as_i64(),
            v["epg_programme_count"].as_i64(),
            v["epg_matched_channels"].as_i64()
        ),
        (Some(1), Some(1), Some(1)),
        "{v}"
    );
    assert!(
        v["last_error"].is_null(),
        "the old EPG error is cleared: {v}"
    );
}

async fn user_app(e: &Env, name: &str) -> (Router, i64) {
    let u = e
        .state
        .auth
        .create_user(reel_auth::NewUser {
            username: name.into(),
            password: "another long password".into(),
            role: reel_auth::Role::User,
            limits: reel_auth::Limits::default(),
            library_ids: vec![],
        })
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login(name, "another long password", Default::default())
        .await
        .unwrap()
        .token;
    (common::with_token(e.raw.clone(), &t), u.id)
}

#[tokio::test]
async fn one_slot_provider_budget_policy_same_user_newest_wins_others_429_admin_takeover() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("a.ts");
    make_ts(&ts, 4);
    let (prov, _) = Provider::start(&ts, 4).await;
    let e = env(|c| c.iptv_grace_secs = 30).await;
    add_source(&e, &prov, Some(1)).await;
    let (news, sport) = (
        channel_id(&e, "UK News HD").await,
        channel_id(&e, "UK Sport").await,
    );
    let (alice1, _) = user_app(&e, "alice").await;
    // a second device of the same user: its own login token
    let t2 = e
        .state
        .auth
        .login("alice", "another long password", Default::default())
        .await
        .unwrap()
        .token;
    let alice2 = common::with_token(e.raw.clone(), &t2);
    let (bob, _) = user_app(&e, "bob").await;
    let tune = |app: &Router, id: i64, body: Value| {
        let app = app.clone();
        async move {
            send(
                &app,
                "POST",
                &format!("/api/v1/iptv/channels/{id}/tune"),
                Some(body),
            )
            .await
        }
    };
    let pt = || json!({"mode": "passthrough"});

    // device 1 watches News; device 2 zaps to Sport: the older session is stopped, no 429
    let (s, a1) = tune(&alice1, news, pt()).await;
    assert_eq!(s, 200, "{a1}");
    let (s, a2) = tune(&alice2, sport, pt()).await;
    assert_eq!(s, 200, "own streams never 429: {a2}");
    assert!(
        e.state
            .iptv
            .live
            .session(a1["session_id"].as_str().unwrap())
            .is_none(),
        "oldest session stopped"
    );
    assert!(
        e.state
            .iptv
            .live
            .session(a2["session_id"].as_str().unwrap())
            .is_some()
    );
    // the same, with HLS sessions (the packager holds the slot)
    let (s, h1) = tune(&alice1, news, json!({"mode":"hls"})).await;
    assert_eq!(s, 200, "{h1}");
    let (s, h2) = tune(&alice2, sport, json!({"mode":"hls"})).await;
    assert_eq!(s, 200, "{h2}");
    assert!(
        e.state
            .iptv
            .live
            .session(h1["session_id"].as_str().unwrap())
            .is_none()
    );
    assert_eq!(
        e.state
            .iptv
            .hub()
            .connections_by_source()
            .values()
            .sum::<usize>(),
        1
    );

    // another user holds it → friendly 429 for non-admins
    let (s, v) = tune(&bob, news, pt()).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("in use by other viewers")
    );
    assert!(
        v.get("holders").is_none(),
        "non-admins do not learn who is watching"
    );
    // … non-admins cannot take over
    let (s, _) = tune(&bob, news, json!({"mode":"passthrough","takeover":true})).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);

    // admins get a 409 with the holders and a takeover hint, and can take over
    let (s, v) = tune(&e.admin, news, pt()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["takeover"], true);
    assert_eq!(v["holders"][0]["username"], "alice");
    assert_eq!(v["holders"][0]["channel_name"], "UK Sport");
    let victim = v["holders"][0]["session_id"].as_str().unwrap().to_string();
    let (s, v) = tune(
        &e.admin,
        news,
        json!({"mode":"passthrough","takeover":true}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        e.state.iptv.live.session(&victim).is_none(),
        "the other stream was stopped"
    );
    let (_, log) = get(&e.admin, "/api/v1/admin/audit").await;
    assert!(log.to_string().contains("iptv_takeover"));

    // a plain DELETE frees the slot at once, even with a long grace configured
    let sid = v["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        send(
            &e.admin,
            "DELETE",
            &format!("/api/v1/iptv/sessions/{sid}"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (s, v) = tune(&bob, sport, pt()).await;
    assert_eq!(s, 200, "{v}");
    e.state.iptv.shutdown().await;
}

#[tokio::test]
async fn sources_take_a_list_of_guide_urls_with_per_guide_stats_and_a_legacy_alias() {
    let e = env(|_| {}).await;
    let guide = |n: &str| format!("http://127.0.0.1:9/{n}.xml.gz"); // never reachable: imports fail per guide
    let (s, v) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/iptv/sources",
        Some(
            json!({"name":"S","kind":"m3u","url":"http://127.0.0.1:9/l.m3u","enabled":false,
            "epg_urls":[guide("AE1"), format!("  {}  ", guide("UK1")), guide("AE1")]}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    // trimmed + de-duplicated, order kept; legacy field = first entry; stats listed before any import
    assert_eq!(v["epg_urls"], json!([guide("AE1"), guide("UK1")]));
    assert_eq!(v["epg_url"], guide("AE1"));
    let es = v["epg_sources"].as_array().unwrap();
    assert_eq!(es.len(), 2);
    assert_eq!(es[1]["url"], guide("UK1"));
    assert_eq!(
        (
            es[0]["channels"].as_i64(),
            es[0]["programmes"].as_i64(),
            es[0]["matched"].as_i64(),
            es[0]["provider"].as_bool()
        ),
        (Some(0), Some(0), Some(0), Some(false))
    );
    let uri = format!("/api/v1/admin/iptv/sources/{id}");
    // PATCH: epg_urls replaces; omitted keeps
    let (s, v) = send(&e.admin, "PATCH", &uri, Some(json!({"name":"S2"}))).await;
    assert_eq!(
        (s, v["epg_urls"].as_array().unwrap().len()),
        (StatusCode::OK, 2)
    );
    let (_, v) = send(
        &e.admin,
        "PATCH",
        &uri,
        Some(json!({"epg_urls":[guide("BEIN1"), guide("AE1"), guide("US")]})),
    )
    .await;
    assert_eq!(
        v["epg_urls"],
        json!([guide("BEIN1"), guide("AE1"), guide("US")])
    );
    // legacy alias: a single epg_url replaces the list; empty clears
    let (_, v) = send(
        &e.admin,
        "PATCH",
        &uri,
        Some(json!({"epg_url": guide("ONLY")})),
    )
    .await;
    assert_eq!(
        (v["epg_urls"].clone(), v["epg_url"].clone()),
        (json!([guide("ONLY")]), json!(guide("ONLY")))
    );
    let (_, v) = send(&e.admin, "PATCH", &uri, Some(json!({"epg_url": ""}))).await;
    assert_eq!(
        (v["epg_urls"].clone(), v["epg_url"].clone()),
        (json!([]), Value::Null)
    );
    assert_eq!(v["epg_sources"], json!([]));
    // validation applies to every entry
    let (s, _) = send(
        &e.admin,
        "PATCH",
        &uri,
        Some(json!({"epg_urls":[guide("A"), "ftp://x/y.xml"]})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // list endpoint carries the same fields
    let (_, l) = get(&e.admin, "/api/v1/admin/iptv/sources").await;
    assert!(l[0]["epg_urls"].is_array() && l[0]["epg_sources"].is_array());
}

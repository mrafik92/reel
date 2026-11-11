mod common;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::State as AxState,
    http::HeaderMap,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use http_body_util::BodyExt;
use reel_api::{AppState, app};
use reel_auth::{Limits, NewUser, Role};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

async fn send(app: &Router, m: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, _, v) = send_h(app, m, uri, body, &[]).await;
    (s, v)
}

async fn send_h(
    app: &Router,
    m: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut r = Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    let b = match body {
        Some(v) => {
            r = r.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let s = res.status();
    let h = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, h, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct Env {
    state: AppState,
    admin: Router,
    raw: Router,
    db: SqlitePool,
    _dir: tempfile::TempDir,
}

async fn env(cast_base: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
    let cfg = reel_core::Config {
        data_dir: dir.path().join("data"),
        cast_base_url: cast_base.map(str::to_string),
        ..Default::default()
    };
    let state = AppState::new(db.clone(), cfg);
    let raw = app(state.clone());
    let admin = common::authed(&state, raw.clone()).await;
    Env {
        state,
        admin,
        raw,
        db,
        _dir: dir,
    }
}

async fn user(e: &Env, name: &str) -> Router {
    let have_lib: i64 = sqlx::query_scalar("SELECT count(*) FROM libraries WHERE id = 1")
        .fetch_one(&e.db)
        .await
        .unwrap();
    let library_ids = if have_lib > 0 { vec![1] } else { vec![] };
    e.state
        .auth
        .create_user(NewUser {
            username: name.into(),
            password: "another long password".into(),
            role: Role::User,
            limits: Limits::default(),
            library_ids,
        })
        .await
        .unwrap();
    // casting is opt-in per user
    sqlx::query("UPDATE users SET can_cast = 1 WHERE username = ?")
        .bind(name)
        .execute(&e.db)
        .await
        .unwrap();
    let t = e
        .state
        .auth
        .login(name, "another long password", Default::default())
        .await
        .unwrap()
        .token;
    common::with_token(e.raw.clone(), &t)
}

type StreamRow<'a> = (
    i64,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    i64,
    i64,
    Option<&'a str>,
);

/// A movie file (mp4/h264) with Arabic default audio, English audio and a set of subtitle tracks.
async fn library_file(e: &Env, real_file: Option<&std::path::Path>) -> i64 {
    sqlx::query("INSERT INTO libraries (id, name, kind) VALUES (1,'M','movies')")
        .execute(&e.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, kind, title) VALUES (1,1,'movie','Polyglot')",
    )
    .execute(&e.db)
    .await
    .unwrap();
    let path = real_file.map_or("/nowhere/polyglot.mp4".to_string(), |p| {
        p.to_string_lossy().into_owned()
    });
    let fid: i64 = sqlx::query_scalar("INSERT INTO media_files (item_id, path, size, mtime, container, duration_s) VALUES (1, ?, 1, 1, 'mp4', 3600) RETURNING id")
        .bind(path).fetch_one(&e.db).await.unwrap();
    // idx, kind, codec, lang, title, default, forced, external
    let rows: [StreamRow; 7] = [
        (0, "video", "h264", None, None, 1, 0, None),
        (1, "audio", "aac", Some("ara"), None, 1, 0, None),
        (2, "audio", "aac", Some("eng"), None, 0, 0, None),
        (
            3,
            "subtitle",
            "subrip",
            Some("eng"),
            Some("English"),
            0,
            0,
            None,
        ),
        (
            4,
            "subtitle",
            "subrip",
            Some("eng"),
            Some("English forced"),
            0,
            1,
            None,
        ),
        (
            5,
            "subtitle",
            "subrip",
            Some("ara"),
            Some("Arabic forced"),
            0,
            1,
            None,
        ),
        (
            6,
            "subtitle",
            "srt",
            Some("en"),
            Some("Polyglot.en.sdh.srt"),
            0,
            0,
            Some("/m/Polyglot.en.sdh.srt"),
        ),
    ];
    for (idx, kind, codec, lang, title, def, forced, ext) in rows {
        let (w, h, ch) = if kind == "video" {
            (Some(128), Some(96), None)
        } else if kind == "audio" {
            (None, None, Some(2))
        } else {
            (None, None, None)
        };
        sqlx::query("INSERT INTO streams (file_id, idx, kind, codec, language, title, \"default\", forced, external_path, width, height, channels) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(fid).bind(idx).bind(kind).bind(codec).bind(lang).bind(title).bind(def).bind(forced).bind(ext).bind(w).bind(h).bind(ch)
            .execute(&e.db).await.unwrap();
    }
    fid
}

type Log = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Clone)]
struct Tv {
    log: Log,
    state: Arc<Mutex<String>>,
}

fn envelope(action: &str, inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">{inner}</u:{action}Response></s:Body></s:Envelope>"
    )
}

async fn av(AxState(tv): AxState<Tv>, h: HeaderMap, body: String) -> (StatusCode, String) {
    let action = h["soapaction"]
        .to_str()
        .unwrap()
        .trim_matches('"')
        .split('#')
        .nth(1)
        .unwrap()
        .to_string();
    tv.log.lock().unwrap().push((action.clone(), body));
    if action == "SetAVTransportURI" && *tv.state.lock().unwrap() == "REJECT" {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><detail><UPnPError><errorCode>714</errorCode><errorDescription>Illegal MIME-Type</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>".into(),
        );
    }
    match action.as_str() {
        "Play" => *tv.state.lock().unwrap() = "PLAYING".into(),
        "Pause" => *tv.state.lock().unwrap() = "PAUSED_PLAYBACK".into(),
        "Stop" if *tv.state.lock().unwrap() != "REJECT" => {
            *tv.state.lock().unwrap() = "STOPPED".into()
        }
        _ => {}
    }
    let inner = match action.as_str() {
        "GetTransportInfo" => format!(
            "<CurrentTransportState>{}</CurrentTransportState>",
            tv.state.lock().unwrap()
        ),
        "GetPositionInfo" => {
            "<TrackDuration>01:00:00</TrackDuration><RelTime>00:01:05</RelTime>".to_string()
        }
        _ => String::new(),
    };
    (StatusCode::OK, envelope(&action, &inner))
}

async fn rc(AxState(tv): AxState<Tv>, h: HeaderMap, body: String) -> (StatusCode, String) {
    let action = h["soapaction"]
        .to_str()
        .unwrap()
        .trim_matches('"')
        .split('#')
        .nth(1)
        .unwrap()
        .to_string();
    tv.log.lock().unwrap().push((format!("RC:{action}"), body));
    (StatusCode::OK, envelope(&action, ""))
}

const DESC: &str = r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><friendlyName>[LG] Living Room</friendlyName>
<manufacturer>LG Electronics</manufacturer><modelName>OLED55C1</modelName><UDN>uuid:1234-abcd</UDN>
<iconList><icon><mimetype>image/png</mimetype><width>120</width><url>/icon.png</url></icon></iconList>
<serviceList>
<service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/ctl/rc</controlURL></service>
<service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/ctl/av</controlURL></service>
</serviceList></device></root>"#;

/// Fake UPnP renderer: description + SOAP endpoints on loopback, and an SSDP responder.
async fn fake_tv() -> (Tv, SocketAddr, SocketAddr) {
    let tv = Tv {
        log: Log::default(),
        state: Arc::new(Mutex::new("STOPPED".into())),
    };
    let app = Router::new()
        .route("/description.xml", get(|| async { DESC }))
        .route("/ctl/av", post(av))
        .route("/ctl/rc", post(rc))
        .with_state(tv.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let ssdp = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((n, from)) = sock.recv_from(&mut buf).await {
            let q = String::from_utf8_lossy(&buf[..n]);
            if q.starts_with("M-SEARCH")
                && q.contains("urn:schemas-upnp-org:device:MediaRenderer:1")
            {
                let r = format!(
                    "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: http://{http}/description.xml\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:1234-abcd::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n"
                );
                let _ = sock.send_to(r.as_bytes(), from).await;
            }
        }
    });
    (tv, http, ssdp)
}

fn actions(tv: &Tv) -> Vec<String> {
    tv.log
        .lock()
        .unwrap()
        .iter()
        .map(|(a, _)| a.clone())
        .collect()
}
fn last(tv: &Tv, action: &str) -> String {
    tv.log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(a, _)| a == action)
        .map(|(_, b)| b.clone())
        .unwrap_or_default()
}

#[test]
fn xml_and_time_helpers() {
    assert_eq!(reel_api::cast::reltime(3725.9), "01:02:05");
    assert_eq!(reel_api::cast::parse_reltime("1:02:05.500"), Some(3725.5));
    assert_eq!(reel_api::cast::parse_reltime("NOT_IMPLEMENTED"), None);
    let d =
        reel_api::cast::parse_description(DESC, "http://10.0.0.5:1999/description.xml").unwrap();
    assert_eq!(
        (d.name.as_str(), d.model.as_deref(), d.ip.as_str()),
        ("[LG] Living Room", Some("OLED55C1"), "10.0.0.5")
    );
    assert_eq!(d.av_control, "http://10.0.0.5:1999/ctl/av");
    assert_eq!(d.rc_control.as_deref(), Some("http://10.0.0.5:1999/ctl/rc"));
    assert_eq!(d.icon.as_deref(), Some("http://10.0.0.5:1999/icon.png"));
    assert!(
        reel_api::cast::parse_description(
            "<root><device><friendlyName>x</friendlyName></device></root>",
            "http://h/"
        )
        .is_none(),
        "no AVTransport → not a renderer"
    );
    // References arrive as separate quick-xml events: spaces around them must survive.
    let d = reel_api::cast::parse_description(
        r#"<root><device><friendlyName> Tom &amp; Jerry&#39;s TV </friendlyName>
<manufacturer>A&lt;B</manufacturer><serviceList><service>
<serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
<controlURL>/ctl?a=1&amp;b=2</controlURL></service></serviceList></device></root>"#,
        "http://10.0.0.6:80/d.xml",
    )
    .unwrap();
    assert_eq!(d.name, "Tom & Jerry's TV");
    assert_eq!(d.av_control, "http://10.0.0.6/ctl?a=1&b=2");
    let m = reel_api::cast::didl(
        "A & B",
        "http://x/y?a=1&b=2",
        "video/mp4",
        true,
        Some("http://x/s.vtt"),
    );
    assert!(
        m.contains("object.item.videoItem") && m.contains("A &amp; B") && m.contains("a=1&amp;b=2")
    );
    assert!(
        m.contains("http-get:*:video/mp4:DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=")
            && m.contains("CaptionInfoEx")
    );
    assert!(reel_api::cast::didl("t", "u", "video/mpeg", false, None).contains("DLNA.ORG_OP=00"));
}

#[tokio::test]
async fn discover_add_by_ip_play_and_control_a_dlna_renderer() {
    let (tv, http, ssdp) = fake_tv().await;
    let e = env(Some("http://10.10.10.207:8420")).await;
    let fid = library_file(&e, None).await;

    // SSDP
    assert_eq!(
        e.state
            .cast
            .discover(ssdp, Duration::from_millis(700))
            .await,
        1
    );
    let (s, d) = send(&e.admin, "GET", "/api/v1/cast/devices", None).await;
    assert_eq!(s, StatusCode::OK);
    let dev = &d[0];
    assert_eq!(d.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            dev["name"].as_str(),
            dev["model"].as_str(),
            dev["protocol"].as_str()
        ),
        (Some("[LG] Living Room"), Some("OLED55C1"), Some("dlna"))
    );
    assert_eq!(dev["supports_volume"], true);
    assert!(dev["icon"].as_str().unwrap().ends_with("/icon.png"));
    let id = dev["id"].as_str().unwrap().to_string();

    // manual add by ip(:port) finds the same device; admin only; nothing there → 404
    let (s, m) = send(
        &e.admin,
        "POST",
        "/api/v1/admin/cast/devices",
        Some(json!({"ip": http.to_string()})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["id"], id);
    assert_eq!(m["manual"], true);
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/cast/devices",
            Some(json!({"ip": "127.0.0.1:1"}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/admin/cast/devices",
            Some(json!({"ip": "a b;"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let dana = user(&e, "dana").await;
    assert_eq!(
        send(
            &dana,
            "POST",
            "/api/v1/admin/cast/devices",
            Some(json!({"ip": "127.0.0.1"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&dana, "GET", "/api/v1/cast/devices", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        send(&e.raw, "GET", "/api/v1/cast/devices", None).await.0,
        StatusCode::UNAUTHORIZED
    );

    // play a library item: absolute signed URL + DIDL-Lite + caption for the auto-selected subtitle
    let play = format!("/api/v1/cast/devices/{id}/play");
    assert_eq!(
        send(
            &e.admin,
            "POST",
            "/api/v1/cast/devices/nope/play",
            Some(json!({"kind":"item","file_id":fid}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &play,
            Some(json!({"kind":"bogus","id":1}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (s, p) = send(
        &e.admin,
        "POST",
        &play,
        Some(json!({"kind":"item","file_id":fid})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let cid = p["cast_session_id"].as_str().unwrap().to_string();
    let acts = actions(&tv);
    assert!(
        acts.ends_with(&["Stop".into(), "SetAVTransportURI".into(), "Play".into()]),
        "{acts:?}"
    );
    let set = last(&tv, "SetAVTransportURI");
    assert!(
        set.contains("<CurrentURI>http://10.10.10.207:8420/api/v1/direct/"),
        "{set}"
    );
    for needle in [
        "object.item.videoItem",
        "http-get:*:video/mp4:DLNA.ORG_OP=01",
        "DLNA.ORG_FLAGS=",
        "CaptionInfoEx",
        "http://10.10.10.207:8420/api/v1/subs/",
        "&lt;dc:title&gt;Polyglot",
    ] {
        assert!(set.contains(needle), "missing {needle} in {set}");
    }
    assert_eq!(
        e.state.playback.list().await.len(),
        1,
        "the cast holds a playback session"
    );

    // status polling
    let (s, st) = send(
        &e.admin,
        "GET",
        &format!("/api/v1/cast/sessions/{cid}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{st}");
    assert_eq!(
        (
            st["state"].as_str(),
            st["position_s"].as_f64(),
            st["duration_s"].as_f64()
        ),
        (Some("playing"), Some(65.0), Some(3600.0))
    );
    // controls
    let c = |a: &str, b: Option<Value>| {
        let app = e.admin.clone();
        let u = format!("/api/v1/cast/sessions/{cid}/{a}");
        async move { send(&app, "POST", &u, b).await.0 }
    };
    assert_eq!(c("pause", None).await, StatusCode::NO_CONTENT);
    assert_eq!(
        send(
            &e.admin,
            "GET",
            &format!("/api/v1/cast/sessions/{cid}"),
            None
        )
        .await
        .1["state"],
        "paused"
    );
    assert_eq!(c("play", None).await, StatusCode::NO_CONTENT);
    assert_eq!(
        c("seek", Some(json!({"position_s": 125}))).await,
        StatusCode::NO_CONTENT
    );
    let seek = last(&tv, "Seek");
    assert!(
        seek.contains("<Unit>REL_TIME</Unit>") && seek.contains("<Target>00:02:05</Target>"),
        "{seek}"
    );
    assert_eq!(c("seek", None).await, StatusCode::BAD_REQUEST);
    assert_eq!(c("rewind", None).await, StatusCode::BAD_REQUEST);
    let (s, _) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/cast/sessions/{cid}/volume"),
        Some(json!({"level": 30})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let vol = last(&tv, "RC:SetVolume");
    assert!(
        vol.contains("<DesiredVolume>30</DesiredVolume>")
            && vol.contains("<Channel>Master</Channel>"),
        "{vol}"
    );
    // other users cannot touch it
    assert_eq!(
        send(&dana, "GET", &format!("/api/v1/cast/sessions/{cid}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &dana,
            "POST",
            &format!("/api/v1/cast/sessions/{cid}/stop"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // casting again to the same TV replaces the first cast (one playback session left)
    let (s, p2) = send(
        &e.admin,
        "POST",
        &play,
        Some(json!({"kind":"item","file_id":fid,"subtitle_index":-1})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p2}");
    assert!(
        !last(&tv, "SetAVTransportURI").contains("CaptionInfoEx"),
        "subtitles explicitly off"
    );
    assert_eq!(e.state.playback.list().await.len(), 1);
    assert_eq!(
        send(
            &e.admin,
            "GET",
            &format!("/api/v1/cast/sessions/{cid}"),
            None
        )
        .await
        .1["state"],
        "stopped"
    );

    // stop: TV stopped, provider/budget session released
    let cid2 = p2["cast_session_id"].as_str().unwrap();
    assert_eq!(
        send(
            &e.admin,
            "POST",
            &format!("/api/v1/cast/sessions/{cid2}/stop"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(*tv.state.lock().unwrap(), "STOPPED");
    assert_eq!(e.state.playback.list().await.len(), 0);
    assert_eq!(
        send(
            &e.admin,
            "GET",
            &format!("/api/v1/cast/sessions/{cid2}"),
            None
        )
        .await
        .1["state"],
        "stopped"
    );
}

#[tokio::test]
async fn casting_needs_a_cast_base_url_and_surfaces_tv_errors() {
    let (_tv, _http, ssdp) = fake_tv().await;
    let e = env(None).await;
    let fid = library_file(&e, None).await;
    assert_eq!(
        e.state
            .cast
            .discover(ssdp, Duration::from_millis(700))
            .await,
        1
    );
    let id = e.state.cast.devices()[0].id.clone();
    let (s, d) = send(
        &e.admin,
        "POST",
        &format!("/api/v1/cast/devices/{id}/play"),
        Some(json!({"kind":"item","file_id":fid})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(d["error"].as_str().unwrap().contains("cast_base_url"));
    // a TV that refuses the media → 502 with its reason, and no leaked playback session
    let e2 = env(Some("http://10.0.0.1:8420")).await;
    let fid2 = library_file(&e2, None).await;
    let (tv2, _http2, ssdp2) = fake_tv().await;
    assert_eq!(
        e2.state
            .cast
            .discover(ssdp2, Duration::from_millis(700))
            .await,
        1
    );
    let id2 = e2.state.cast.devices()[0].id.clone();
    *tv2.state.lock().unwrap() = "REJECT".into();
    let (s, d) = send(
        &e2.admin,
        "POST",
        &format!("/api/v1/cast/devices/{id2}/play"),
        Some(json!({"kind":"item","file_id":fid2})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{d}");
    assert!(
        d["error"].as_str().unwrap().contains("Illegal MIME-Type"),
        "{d}"
    );
    assert_eq!(e2.state.playback.list().await.len(), 0);
}

//! DLNA / UPnP "Play on TV": SSDP discovery of MediaRenderers, SOAP control (AVTransport +
//! RenderingControl) and cast sessions that share the provider-budget rules of normal playback.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::UdpSocket;
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::{AdminUser, AuthUser},
    error::{ApiError, ApiResult, ErrorBody},
};

pub const SSDP_ADDR: &str = "239.255.255.250:1900";
const RENDERER_ST: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
/// Non-manual devices that stopped answering searches for this long are dropped.
const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
const REFRESH_EVERY: Duration = Duration::from_secs(60);

// ───────────────────────── model ─────────────────────────

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub icon: Option<String>,
    pub ip: String,
    pub location: String,
    pub av_control: String,
    pub av_type: String,
    pub rc_control: Option<String>,
    pub rc_type: Option<String>,
    pub manual: bool,
    pub last_seen: Instant,
}

/// A TV / speaker that can be cast to.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CastDeviceDto {
    /// Stable id (derived from the device UDN).
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Absolute icon URL on the device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub ip: String,
    /// Always `dlna` here.
    pub protocol: String,
    pub supports_volume: bool,
    /// Added by IP rather than found by SSDP.
    pub manual: bool,
}

impl From<&Device> for CastDeviceDto {
    fn from(d: &Device) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            model: d.model.clone(),
            manufacturer: d.manufacturer.clone(),
            icon: d.icon.clone(),
            ip: d.ip.clone(),
            protocol: "dlna".into(),
            supports_volume: d.rc_control.is_some(),
            manual: d.manual,
        }
    }
}

struct CastSess {
    user_id: i64,
    device_id: String,
    /// Playback session (library / VOD / live) whose provider budget this cast holds.
    playback_id: String,
    duration_s: Option<f64>,
    started: Instant,
    ended: bool,
}

pub struct CastRuntime {
    devices: Mutex<HashMap<String, Device>>,
    sessions: Mutex<HashMap<String, CastSess>>,
    http: reqwest::Client,
}

impl Default for CastRuntime {
    fn default() -> Self {
        Self {
            devices: Mutex::default(),
            sessions: Mutex::default(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .build()
                .unwrap_or_default(),
        }
    }
}

// ───────────────────────── XML helpers ─────────────────────────

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Text of the first `<name>` / `<ns:name>` element (unescaped).
fn tag_text(xml: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(p) = xml[from..].find(name) {
        let at = from + p;
        let before = xml[..at].chars().next_back();
        let after = xml[at + name.len()..].chars().next();
        if matches!(before, Some('<') | Some(':')) && after == Some('>') {
            let start = at + name.len() + 1;
            let end = xml[start..].find("</")?;
            return Some(xml_unescape(xml[start..start + end].trim()));
        }
        from = at + name.len();
    }
    None
}

fn resolve(base: &str, rel: &str) -> Option<String> {
    reqwest::Url::parse(base)
        .ok()?
        .join(rel)
        .ok()
        .map(|u| u.to_string())
}

/// Parse a UPnP device description. `None` when it is not a renderer with AVTransport.
pub fn parse_description(xml: &str, location: &str) -> Option<Device> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(true);
    let mut stack: Vec<String> = vec![];
    let (mut name, mut model, mut manu, mut udn, mut icon, mut urlbase) =
        (None, None, None, None, None, None::<String>);
    let (mut st, mut ctl) = (None::<String>, None::<String>);
    let (mut av, mut rc) = (None::<(String, String)>, None::<(String, String)>);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                stack.push(String::from_utf8_lossy(e.local_name().as_ref()).into_owned())
            }
            Ok(Event::End(_)) => {
                if stack.pop().as_deref() == Some("service") {
                    if let (Some(t), Some(c)) = (st.take(), ctl.take()) {
                        if t.contains(":AVTransport:") {
                            av = av.or(Some((t, c)));
                        } else if t.contains(":RenderingControl:") {
                            rc = rc.or(Some((t, c)));
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                let Ok(txt) = t.unescape() else { continue };
                let txt = txt.to_string();
                let n = stack.len();
                let cur = stack.last().map(String::as_str).unwrap_or("");
                let parent = n
                    .checked_sub(2)
                    .and_then(|i| stack.get(i))
                    .map(String::as_str);
                match cur {
                    "friendlyName" if name.is_none() => name = Some(txt),
                    "modelName" if model.is_none() => model = Some(txt),
                    "manufacturer" if manu.is_none() => manu = Some(txt),
                    "UDN" if udn.is_none() => udn = Some(txt),
                    "URLBase" if urlbase.is_none() => urlbase = Some(txt),
                    "serviceType" => st = Some(txt),
                    "controlURL" => ctl = Some(txt),
                    "url" if parent == Some("icon") && icon.is_none() => icon = Some(txt),
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let base = urlbase
        .filter(|b| b.starts_with("http"))
        .unwrap_or_else(|| location.to_string());
    let (av_type, av_ctl) = av?;
    let host = reqwest::Url::parse(location).ok()?.host_str()?.to_string();
    let udn = udn.unwrap_or_else(|| location.to_string());
    let mut h = Sha256::new();
    h.update(udn.trim_start_matches("uuid:").as_bytes());
    let id = format!(
        "dlna-{}",
        h.finalize()[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    Some(Device {
        id,
        name: name.unwrap_or_else(|| host.clone()),
        model,
        manufacturer: manu,
        icon: icon.and_then(|i| resolve(&base, &i)),
        ip: host,
        location: location.to_string(),
        av_control: resolve(&base, &av_ctl)?,
        av_type,
        rc_control: rc.as_ref().and_then(|(_, c)| resolve(&base, c)),
        rc_type: rc.map(|(t, _)| t),
        manual: false,
        last_seen: Instant::now(),
    })
}

// ───────────────────────── discovery ─────────────────────────

fn header_value(msg: &str, name: &str) -> Option<String> {
    msg.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// Common description URLs of TVs that do not answer (or are not reachable by) multicast.
fn probe_candidates(ip: &str) -> Vec<String> {
    const PATHS: &[&str] = &["/description.xml", "/dmr", "/"];
    if let Some((h, port)) = ip.rsplit_once(':') {
        if !h.is_empty() && !h.contains(']') && port.chars().all(|c| c.is_ascii_digit()) {
            return PATHS.iter().map(|p| format!("http://{ip}{p}")).collect();
        }
    }
    let mut v: Vec<String> = vec![
        format!("http://{ip}:9197/dmr"),      // Samsung
        format!("http://{ip}:7676/smp_2_"),   // Samsung (older)
        format!("http://{ip}:55000/dmr"),     // Panasonic / Samsung
        format!("http://{ip}:52323/dmr.xml"), // Sony
        format!("http://{ip}:49152/description.xml"),
        format!("http://{ip}:49153/description.xml"),
        format!("http://{ip}:8008/ssdp/device-desc.xml"),
        format!("http://{ip}:1400/xml/device_description.xml"),
    ];
    // LG webOS answers on 1x99 (1099 … 1999)
    for x in 0..=9 {
        v.push(format!("http://{ip}:1{x}99/"));
        v.push(format!("http://{ip}:1{x}99/dmr"));
    }
    v.push(format!("http://{ip}:1990/"));
    v
}

impl CastRuntime {
    pub fn devices(&self) -> Vec<Device> {
        let mut v: Vec<Device> = self.devices.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        v
    }

    pub fn device(&self, id: &str) -> Option<Device> {
        self.devices.lock().unwrap().get(id).cloned()
    }

    fn upsert(&self, mut d: Device) -> Device {
        let mut m = self.devices.lock().unwrap();
        if let Some(old) = m.get(&d.id) {
            d.manual |= old.manual;
        }
        m.insert(d.id.clone(), d.clone());
        d
    }

    /// Fetch and register the renderer described at `location`.
    pub async fn add_location(&self, location: &str, manual: bool) -> Result<Device, String> {
        let res = self
            .http
            .get(location)
            .timeout(Duration::from_millis(2500))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("HTTP {}", res.status()));
        }
        let xml = res.text().await.map_err(|e| e.to_string())?;
        let mut d = parse_description(&xml, location).ok_or("not a MediaRenderer")?;
        d.manual = manual;
        Ok(self.upsert(d))
    }

    /// Probe the usual description URLs of `ip` (or `ip:port`).
    pub async fn add_by_ip(&self, ip: &str) -> Option<Device> {
        let tries = probe_candidates(ip)
            .into_iter()
            .map(|u| async move { self.add_location(&u, true).await.ok() });
        futures_util::future::join_all(tries)
            .await
            .into_iter()
            .flatten()
            .next()
    }

    /// One SSDP round: M-SEARCH to `target`, collect answers for `wait`, register the renderers.
    pub async fn discover(&self, target: SocketAddr, wait: Duration) -> usize {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else {
            return 0;
        };
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {RENDERER_ST}\r\n\r\n"
        );
        for _ in 0..2 {
            let _ = sock.send_to(msg.as_bytes(), target).await;
        }
        let mut locations: HashSet<String> = HashSet::new();
        let deadline = tokio::time::Instant::now() + wait;
        let mut buf = vec![0u8; 4096];
        while let Ok(Ok((n, _))) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await
        {
            let text = String::from_utf8_lossy(&buf[..n]);
            if let Some(l) = header_value(&text, "location") {
                if l.starts_with("http") {
                    locations.insert(l);
                }
            }
        }
        let found = futures_util::future::join_all(
            locations
                .iter()
                .map(|l| async move { self.add_location(l, false).await.ok() }),
        )
        .await;
        found.into_iter().flatten().count()
    }

    fn prune(&self) {
        self.devices
            .lock()
            .unwrap()
            .retain(|_, d| d.manual || d.last_seen.elapsed() < STALE_AFTER);
    }

    /// Background discovery: manual devices from `data_dir/cast_devices.json`, then SSDP every minute.
    pub fn spawn_discovery(self: &Arc<Self>, data_dir: PathBuf) {
        let me = self.clone();
        tokio::spawn(async move {
            let file = data_dir.join("cast_devices.json");
            if let Ok(t) = tokio::fs::read_to_string(&file).await {
                for loc in serde_json::from_str::<Vec<String>>(&t).unwrap_or_default() {
                    let _ = me.add_location(&loc, true).await;
                }
            }
            let target: SocketAddr = SSDP_ADDR.parse().expect("ssdp addr");
            loop {
                let n = me.discover(target, Duration::from_secs(3)).await;
                tracing::debug!(found = n, "ssdp search");
                me.prune();
                tokio::time::sleep(REFRESH_EVERY).await;
            }
        });
    }

    fn save_manual(&self, data_dir: &std::path::Path) {
        let locs: Vec<String> = self
            .devices()
            .into_iter()
            .filter(|d| d.manual)
            .map(|d| d.location)
            .collect();
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(
            data_dir.join("cast_devices.json"),
            serde_json::to_string(&locs).unwrap_or_default(),
        );
    }

    // ───────────── SOAP ─────────────

    async fn soap(
        &self,
        url: &str,
        service: &str,
        action: &str,
        args: &[(&str, String)],
    ) -> ApiResult<String> {
        let mut body = String::new();
        for (k, v) in args {
            body.push_str(&format!("<{k}>{}</{k}>", xml_escape(v)));
        }
        let env = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{body}</u:{action}></s:Body></s:Envelope>"
        );
        let res = self
            .http
            .post(url)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SOAPACTION", format!("\"{service}#{action}\""))
            .body(env)
            .send()
            .await
            .map_err(|e| ApiError::BadGateway(format!("TV unreachable: {e}")))?;
        let ok = res.status().is_success();
        let text = res.text().await.unwrap_or_default();
        if !ok {
            let why = tag_text(&text, "errorDescription")
                .or_else(|| tag_text(&text, "faultstring"))
                .unwrap_or_else(|| "request refused".into());
            return Err(ApiError::BadGateway(format!("TV rejected {action}: {why}")));
        }
        Ok(text)
    }

    async fn av(&self, d: &Device, action: &str, args: &[(&str, String)]) -> ApiResult<String> {
        let mut a = vec![("InstanceID", "0".to_string())];
        a.extend(args.iter().map(|(k, v)| (*k, v.clone())));
        self.soap(&d.av_control, &d.av_type, action, &a).await
    }
}

pub fn reltime(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
}

pub fn parse_reltime(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    for p in parts {
        total = total * 60.0 + p.parse::<f64>().ok()?;
    }
    Some(total)
}

/// DIDL-Lite metadata for `SetAVTransportURI`.
pub fn didl(title: &str, url: &str, mime: &str, seekable: bool, caption: Option<&str>) -> String {
    let (op, flags) = if seekable {
        ("01", "01700000000000000000000000000000")
    } else {
        ("00", "0D700000000000000000000000000000")
    };
    let mut s = format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\" \
         xmlns:sec=\"http://www.sec.co.kr/\"><item id=\"0\" parentID=\"-1\" restricted=\"1\">\
         <dc:title>{}</dc:title><upnp:class>object.item.videoItem</upnp:class>\
         <res protocolInfo=\"http-get:*:{}:DLNA.ORG_OP={op};DLNA.ORG_CI=0;DLNA.ORG_FLAGS={flags}\"",
        xml_escape(title),
        xml_escape(mime),
    );
    if let Some(c) = caption {
        s.push_str(&format!(" sec:URI=\"{}\"", xml_escape(c)));
    }
    s.push_str(&format!(">{}</res>", xml_escape(url)));
    if let Some(c) = caption {
        s.push_str(&format!(
            "<res protocolInfo=\"http-get:*:text/vtt:*\">{0}</res>\
             <sec:CaptionInfoEx sec:type=\"vtt\">{0}</sec:CaptionInfoEx>\
             <sec:CaptionInfo sec:type=\"vtt\">{0}</sec:CaptionInfo>",
            xml_escape(c)
        ));
    }
    s.push_str("</item></DIDL-Lite>");
    s
}

// ───────────────────────── API ─────────────────────────

/// DLNA renderers (TVs) currently known: found by SSDP (refreshed every minute) or added by IP.
#[utoipa::path(get, path = "/cast/devices", responses((status = 200, body = Vec<CastDeviceDto>)))]
pub async fn list_devices(State(st): State<AppState>, _u: AuthUser) -> Json<Vec<CastDeviceDto>> {
    Json(st.cast.devices().iter().map(Into::into).collect())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddDeviceRequest {
    /// TV address: `192.168.1.40`, or `192.168.1.40:9197` to probe one port only.
    pub ip: String,
}

/// Add a TV by IP: probes the usual description URLs (Samsung `:9197/dmr`, LG `:1x99/`, …).
/// The device is remembered across restarts.
#[utoipa::path(
    post,
    path = "/admin/cast/devices",
    request_body = AddDeviceRequest,
    responses((status = 200, body = CastDeviceDto), (status = 404, description = "No renderer answered", body = ErrorBody))
)]
pub async fn add_device(
    State(st): State<AppState>,
    _a: AdminUser,
    Json(req): Json<AddDeviceRequest>,
) -> ApiResult<Json<CastDeviceDto>> {
    let ip = req.ip.trim();
    if ip.is_empty()
        || ip.len() > 64
        || !ip
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".:-[]".contains(c))
    {
        return Err(ApiError::BadRequest("invalid ip".into()));
    }
    let d = st.cast.add_by_ip(ip).await.ok_or_else(|| {
        ApiError::Status(
            StatusCode::NOT_FOUND,
            format!("no UPnP MediaRenderer answered at {ip}"),
        )
    })?;
    st.cast.save_manual(&st.config.data_dir);
    Ok(Json(CastDeviceDto::from(&d)))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CastPlayRequest {
    /// `item` (library file), `vod` (provider movie/episode) or `channel` (live TV).
    pub kind: String,
    /// Item: file id (or `file_id`). Vod: vod movie/episode id. Channel: channel id.
    pub id: Option<i64>,
    pub file_id: Option<i64>,
    /// Vod only: `movie` (default) or `episode`.
    pub vod_kind: Option<String>,
    pub start_s: Option<f64>,
    pub audio_index: Option<i64>,
    /// `-1` = no subtitles, omitted = automatic.
    pub subtitle_index: Option<i64>,
    /// Admins only: take over a provider slot held by other users (answer to a 409).
    pub takeover: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CastPlayResponse {
    pub cast_session_id: String,
}

struct Resolved {
    session_id: String,
    url: String,
    mime: String,
    title: String,
    duration_s: Option<f64>,
    seekable: bool,
    caption: Option<String>,
    seek_to: Option<f64>,
}

fn mime_for(hls: bool, container: &str) -> String {
    if hls {
        "application/vnd.apple.mpegurl".into()
    } else {
        match container {
            "mpegts" | "ts" => "video/mpeg".into(),
            c => match crate::routes::container_mime(c) {
                "application/octet-stream" => "video/mp4".into(),
                m => m.into(),
            },
        }
    }
}

async fn one_str(st: &AppState, sql: &str, id: i64) -> Option<String> {
    sqlx::query_scalar::<_, String>(sql)
        .bind(id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
}

/// Start something on a TV. Uses the TV-friendly `dlna` preset and absolute signed URLs on
/// `cast_base_url`; provider-budget rules are those of the normal playback endpoints (same-user
/// newest wins, 429, admin 409 + `takeover`).
#[utoipa::path(
    post,
    path = "/cast/devices/{id}/play",
    params(("id" = String, Path, description = "Device id from /cast/devices")),
    request_body = CastPlayRequest,
    responses(
        (status = 200, body = CastPlayResponse),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
        (status = 429, body = ErrorBody),
        (status = 502, description = "The TV rejected or did not answer", body = ErrorBody)
    )
)]
pub async fn play_on_device(
    State(st): State<AppState>,
    u: AuthUser,
    Path(dev_id): Path<String>,
    Json(req): Json<CastPlayRequest>,
) -> ApiResult<Json<CastPlayResponse>> {
    let dev = st.cast.device(&dev_id).ok_or(ApiError::NotFound)?;
    crate::prefs::cast_ctx(&st, true)?;
    let uid = u.id();
    // one cast per TV: the previous one makes room
    let old: Vec<String> = st
        .cast
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| s.device_id == dev.id && !s.ended)
        .map(|(k, _)| k.clone())
        .collect();
    let r = resolve_request(&st, u, &req).await?;
    for o in old {
        end_cast(&st, &o, false).await;
    }
    let meta = didl(&r.title, &r.url, &r.mime, r.seekable, r.caption.as_deref());
    let started = async {
        let _ = st.cast.av(&dev, "Stop", &[]).await;
        st.cast
            .av(
                &dev,
                "SetAVTransportURI",
                &[("CurrentURI", r.url.clone()), ("CurrentURIMetaData", meta)],
            )
            .await?;
        st.cast.av(&dev, "Play", &[("Speed", "1".into())]).await?;
        if let Some(s) = r.seek_to.filter(|s| *s > 1.0) {
            // renderers need a moment before they accept a seek
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = st
                .cast
                .av(
                    &dev,
                    "Seek",
                    &[("Unit", "REL_TIME".into()), ("Target", reltime(s))],
                )
                .await;
        }
        ApiResult::Ok(())
    }
    .await;
    if let Err(e) = started {
        release(&st, &r.session_id).await;
        return Err(e);
    }
    let id = cast_id(&r.session_id);
    st.cast.sessions.lock().unwrap().insert(
        id.clone(),
        CastSess {
            user_id: uid,
            device_id: dev.id.clone(),
            playback_id: r.session_id,
            duration_s: r.duration_s,
            started: Instant::now(),
            ended: false,
        },
    );
    Ok(Json(CastPlayResponse {
        cast_session_id: id,
    }))
}

fn cast_id(seed: &str) -> String {
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    format!(
        "cast-{}",
        h.finalize()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

async fn release(st: &AppState, playback_id: &str) {
    st.end_session(playback_id).await;
    st.iptv.live.stop_and_release(playback_id).await;
}

async fn resolve_request(st: &AppState, u: AuthUser, req: &CastPlayRequest) -> ApiResult<Resolved> {
    match req.kind.as_str() {
        "item" => {
            let fid = req
                .file_id
                .or(req.id)
                .ok_or_else(|| ApiError::BadRequest("file_id required".into()))?;
            let r = crate::playback::decide_playback(
                State(st.clone()),
                u,
                Path(fid),
                Json(crate::playback::DecideRequest {
                    profile: None,
                    preset: Some("dlna".into()),
                    audio_index: req.audio_index,
                    subtitle_index: req.subtitle_index,
                    start_s: req.start_s,
                    device_id: None,
                    item_id: None,
                    for_cast: Some(true),
                }),
            )
            .await?
            .0;
            let title = one_str(
                st,
                "SELECT i.title FROM media_items i JOIN media_files f ON f.item_id = i.id WHERE f.id = ?",
                fid,
            )
            .await
            .unwrap_or_else(|| "Reel".into());
            let (dur, container): (Option<f64>, Option<String>) =
                sqlx::query_as("SELECT duration_s, container FROM media_files WHERE id = ?")
                    .bind(fid)
                    .fetch_optional(&st.db)
                    .await?
                    .unwrap_or((None, None));
            Ok(finish(
                &r,
                title,
                dur,
                container.as_deref().unwrap_or(""),
                req.start_s,
            ))
        }
        "vod" => {
            let id = req
                .id
                .ok_or_else(|| ApiError::BadRequest("id required".into()))?;
            let kind = req.vod_kind.clone().unwrap_or_else(|| "movie".into());
            let title = if kind == "episode" {
                one_str(st, "SELECT title FROM vod_episodes WHERE id = ?", id).await
            } else {
                one_str(st, "SELECT title FROM vod_movies WHERE id = ?", id).await
            }
            .unwrap_or_else(|| "Reel".into());
            let r = crate::vod::vod_play(
                State(st.clone()),
                u,
                Json(crate::vod::VodPlayRequest {
                    kind,
                    id,
                    profile: None,
                    preset: Some("dlna".into()),
                    audio_index: req.audio_index,
                    subtitle_index: req.subtitle_index,
                    start_s: req.start_s,
                    device_id: None,
                    takeover: req.takeover,
                    for_cast: Some(true),
                }),
            )
            .await?
            .0;
            let vs = st.vod.get(&r.session_id);
            let container = vs.as_ref().map(|v| v.container.clone()).unwrap_or_default();
            Ok(finish(
                &r,
                title,
                vs.and_then(|v| v.duration_s),
                &container,
                req.start_s,
            ))
        }
        "channel" => {
            let id = req
                .id
                .ok_or_else(|| ApiError::BadRequest("id required".into()))?;
            let title = one_str(st, "SELECT name FROM channels WHERE id = ?", id)
                .await
                .unwrap_or_else(|| "Live TV".into());
            let t = crate::iptv::tune(
                State(st.clone()),
                u,
                Path(id),
                Some(Json(crate::iptv::TuneBody {
                    mode: Some(crate::iptv::LiveModeDto::Passthrough),
                    preset: Some("dlna".into()),
                    profile: None,
                    takeover: req.takeover,
                    for_cast: Some(true),
                })),
            )
            .await?
            .0;
            let hls = t.url.contains(".m3u8");
            Ok(Resolved {
                session_id: t.session_id,
                url: t.url,
                mime: if hls {
                    "application/vnd.apple.mpegurl".into()
                } else {
                    "video/mpeg".into()
                },
                title,
                duration_s: None,
                seekable: false,
                caption: None,
                seek_to: None,
            })
        }
        o => Err(ApiError::BadRequest(format!(
            "unknown kind '{o}' (item | vod | channel)"
        ))),
    }
}

fn finish(
    r: &crate::playback::DecideResponse,
    title: String,
    dur: Option<f64>,
    container: &str,
    start_s: Option<f64>,
) -> Resolved {
    use reel_playback::PlaybackMode;
    let direct = r.mode == PlaybackMode::DirectPlay;
    let caption = if direct {
        r.selected
            .subtitle_index
            .and_then(|i| r.subtitles.iter().find(|s| s.index == i))
            .and_then(|s| s.url.clone())
    } else {
        None
    };
    Resolved {
        session_id: r.session_id.clone(),
        url: r.url.clone(),
        mime: mime_for(!direct, container),
        title,
        duration_s: dur,
        seekable: direct,
        caption,
        seek_to: if direct { start_s } else { None },
    }
}

async fn end_cast(st: &AppState, id: &str, stop_tv: bool) {
    let sess = {
        let mut m = st.cast.sessions.lock().unwrap();
        match m.get_mut(id) {
            Some(s) if !s.ended => {
                s.ended = true;
                Some((s.playback_id.clone(), s.device_id.clone()))
            }
            _ => None,
        }
    };
    if let Some((pid, did)) = sess {
        if stop_tv {
            if let Some(d) = st.cast.device(&did) {
                let _ = st.cast.av(&d, "Stop", &[]).await;
            }
        }
        release(st, &pid).await;
    }
}

struct Owned {
    dev: Device,
    duration: Option<f64>,
    age: f64,
    ended: bool,
    playback_id: String,
}

fn own(st: &AppState, u: &AuthUser, id: &str) -> ApiResult<Owned> {
    let m = st.cast.sessions.lock().unwrap();
    let s = m.get(id).ok_or(ApiError::NotFound)?;
    if s.user_id != u.id() && !u.is_admin() {
        return Err(ApiError::NotFound);
    }
    let dev = st.cast.device(&s.device_id).ok_or(ApiError::NotFound)?;
    Ok(Owned {
        dev,
        duration: s.duration_s,
        age: s.started.elapsed().as_secs_f64(),
        ended: s.ended,
        playback_id: s.playback_id.clone(),
    })
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CastSessionState {
    /// `playing` | `paused` | `buffering` | `stopped`
    pub state: String,
    pub position_s: f64,
    pub duration_s: Option<f64>,
}

/// Poll the TV (GetTransportInfo + GetPositionInfo). A TV that went back to idle ends the cast and
/// frees its provider slot.
#[utoipa::path(
    get,
    path = "/cast/sessions/{id}",
    params(("id" = String, Path, description = "cast_session_id")),
    responses((status = 200, body = CastSessionState), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody))
)]
pub async fn get_session(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<CastSessionState>> {
    let o = own(&st, &u, &id)?;
    if o.ended {
        return Ok(Json(CastSessionState {
            state: "stopped".into(),
            position_s: 0.0,
            duration_s: o.duration,
        }));
    }
    let ti = st.cast.av(&o.dev, "GetTransportInfo", &[]).await?;
    let raw = tag_text(&ti, "CurrentTransportState")
        .unwrap_or_default()
        .to_uppercase();
    let state = match raw.as_str() {
        "PLAYING" => "playing",
        "PAUSED_PLAYBACK" | "PAUSED_RECORDING" => "paused",
        "TRANSITIONING" => "buffering",
        _ => "stopped",
    };
    let pos = st.cast.av(&o.dev, "GetPositionInfo", &[]).await.ok();
    let position_s = pos
        .as_deref()
        .and_then(|p| tag_text(p, "RelTime"))
        .and_then(|t| parse_reltime(&t))
        .unwrap_or(0.0);
    let duration_s = pos
        .as_deref()
        .and_then(|p| tag_text(p, "TrackDuration"))
        .and_then(|t| parse_reltime(&t))
        .filter(|d| *d > 0.0)
        .or(o.duration);
    if state == "stopped" && o.age > 20.0 {
        end_cast(&st, &id, false).await;
    } else {
        // the TV is the player: this poll is the "sender ping" that keeps the session alive
        let _ = st
            .playback
            .update_progress(&o.playback_id, position_s, None);
    }
    Ok(Json(CastSessionState {
        state: state.into(),
        position_s,
        duration_s,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ControlBody {
    /// `seek` only: target position in seconds.
    pub position_s: Option<f64>,
}

/// `play` | `pause` | `stop` | `seek` (body `{position_s}`). `stop` also releases the provider slot.
#[utoipa::path(
    post,
    path = "/cast/sessions/{id}/{action}",
    params(
        ("id" = String, Path, description = "cast_session_id"),
        ("action" = String, Path, description = "play | pause | stop | seek")
    ),
    request_body(content = Option<ControlBody>, description = "seek: {position_s}"),
    responses((status = 204, description = "Done"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody))
)]
pub async fn control(
    State(st): State<AppState>,
    u: AuthUser,
    Path((id, action)): Path<(String, String)>,
    body: Option<Json<ControlBody>>,
) -> ApiResult<StatusCode> {
    let o = own(&st, &u, &id)?;
    if action == "stop" {
        end_cast(&st, &id, true).await;
        return Ok(StatusCode::NO_CONTENT);
    }
    if o.ended {
        return Err(ApiError::BadRequest("the cast has ended".into()));
    }
    match action.as_str() {
        "play" => {
            st.cast.av(&o.dev, "Play", &[("Speed", "1".into())]).await?;
        }
        "pause" => {
            st.cast.av(&o.dev, "Pause", &[]).await?;
        }
        "seek" => {
            let p = body
                .and_then(|b| b.0.position_s)
                .filter(|p| p.is_finite() && *p >= 0.0)
                .ok_or_else(|| ApiError::BadRequest("position_s required".into()))?;
            st.cast
                .av(
                    &o.dev,
                    "Seek",
                    &[("Unit", "REL_TIME".into()), ("Target", reltime(p))],
                )
                .await?;
        }
        x => {
            return Err(ApiError::BadRequest(format!(
                "unknown action '{x}' (play | pause | stop | seek)"
            )));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VolumeBody {
    /// 0–100
    pub level: u32,
}

/// Set the TV volume (RenderingControl).
#[utoipa::path(
    post,
    path = "/cast/sessions/{id}/volume",
    params(("id" = String, Path, description = "cast_session_id")),
    request_body = VolumeBody,
    responses((status = 204, description = "Done"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody))
)]
pub async fn set_volume(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<String>,
    Json(b): Json<VolumeBody>,
) -> ApiResult<StatusCode> {
    let o = own(&st, &u, &id)?;
    let (Some(url), Some(ty)) = (&o.dev.rc_control, &o.dev.rc_type) else {
        return Err(ApiError::BadRequest("this TV has no volume control".into()));
    };
    st.cast
        .soap(
            url,
            ty,
            "SetVolume",
            &[
                ("InstanceID", "0".into()),
                ("Channel", "Master".into()),
                ("DesiredVolume", b.level.min(100).to_string()),
            ],
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

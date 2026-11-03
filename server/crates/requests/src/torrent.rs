//! Optional direct torrent-client adapters (qBittorrent WebUI v2, Transmission RPC).
//!
//! Matched to Arr queue items by `downloadId` (torrent infohash, case-insensitive).

use std::{collections::HashMap, time::Duration};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{Error, Result};

/// Fine-grained torrent data.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentInfo {
    /// Lower-case infohash.
    pub hash: String,
    /// 0..1
    pub progress: f32,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub seeds: u32,
    pub peers: u32,
    pub size_bytes: u64,
    pub state: String,
}

/// `download_clients` configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentClientConfig {
    pub kind: TorrentKind,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentKind {
    Qbittorrent,
    Transmission,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

/// A configured adapter.
#[derive(Debug)]
pub enum TorrentClient {
    Qbittorrent(Qbittorrent),
    Transmission(Transmission),
}

impl TorrentClient {
    pub fn from_config(c: &TorrentClientConfig) -> Self {
        match c.kind {
            TorrentKind::Qbittorrent => TorrentClient::Qbittorrent(Qbittorrent::new(
                &c.url,
                c.username.clone(),
                c.password.clone(),
            )),
            TorrentKind::Transmission => TorrentClient::Transmission(Transmission::new(
                &c.url,
                c.username.clone(),
                c.password.clone(),
            )),
        }
    }

    /// Info for the given infohashes, keyed by lower-case hash.
    pub async fn info(&self, hashes: &[String]) -> Result<HashMap<String, TorrentInfo>> {
        match self {
            TorrentClient::Qbittorrent(q) => q.info(hashes).await,
            TorrentClient::Transmission(t) => t.info(hashes).await,
        }
    }
}

/// qBittorrent WebUI API v2.
#[derive(Debug)]
pub struct Qbittorrent {
    http: reqwest::Client,
    base: String,
    username: Option<String>,
    password: Option<String>,
    cookie: Mutex<Option<String>>,
}

impl Qbittorrent {
    pub fn new(base: &str, username: Option<String>, password: Option<String>) -> Self {
        Self {
            http: http(),
            base: base.trim_end_matches('/').to_string(),
            username,
            password,
            cookie: Mutex::new(None),
        }
    }

    async fn login(&self) -> Result<Option<String>> {
        let Some(user) = self.username.as_deref().filter(|u| !u.is_empty()) else {
            return Ok(None);
        };
        let resp = self
            .http
            .post(format!("{}/api/v2/auth/login", self.base))
            .header("Referer", &self.base)
            .form(&[
                ("username", user),
                ("password", self.password.as_deref().unwrap_or("")),
            ])
            .send()
            .await?;
        let status = resp.status();
        let cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            // qBittorrent < 5 names the session cookie `SID`; 5.x uses `QBT_SID_<port>`.
            .find_map(|c| c.split(';').next().filter(|p| is_session_cookie(p)))
            .map(str::to_string);
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() || body.trim().eq_ignore_ascii_case("fails.") {
            return Err(Error::Remote {
                status: status.as_u16(),
                body: format!("qBittorrent login failed: {body}"),
            });
        }
        Ok(cookie)
    }

    async fn fetch(&self, hashes: &[String], cookie: Option<&str>) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .get(format!("{}/api/v2/torrents/info", self.base))
            .query(&[("hashes", hashes.join("|"))]);
        if let Some(c) = cookie {
            req = req.header(reqwest::header::COOKIE, c);
        }
        Ok(req.send().await?)
    }

    pub async fn info(&self, hashes: &[String]) -> Result<HashMap<String, TorrentInfo>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let mut guard = self.cookie.lock().await;
        if guard.is_none() {
            *guard = self.login().await?;
        }
        let mut resp = self.fetch(hashes, guard.as_deref()).await?;
        if resp.status().as_u16() == 403 {
            *guard = self.login().await?;
            resp = self.fetch(hashes, guard.as_deref()).await?;
        }
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Remote {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        let v: Value = resp.json().await?;
        Ok(parse_qbittorrent(&v))
    }
}

/// Parse qBittorrent `/torrents/info` JSON.
pub fn parse_qbittorrent(v: &Value) -> HashMap<String, TorrentInfo> {
    let mut out = HashMap::new();
    for t in v.as_array().into_iter().flatten() {
        let Some(hash) = t.get("hash").and_then(Value::as_str) else {
            continue;
        };
        let hash = hash.to_ascii_lowercase();
        let eta = t
            .get("eta")
            .and_then(Value::as_u64)
            .filter(|e| *e < 8_640_000);
        out.insert(
            hash.clone(),
            TorrentInfo {
                hash,
                progress: t.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                speed_bps: t.get("dlspeed").and_then(Value::as_u64).unwrap_or(0),
                eta_seconds: eta,
                seeds: t.get("num_seeds").and_then(Value::as_u64).unwrap_or(0) as u32,
                peers: t.get("num_leechs").and_then(Value::as_u64).unwrap_or(0) as u32,
                size_bytes: t
                    .get("total_size")
                    .or_else(|| t.get("size"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                state: t
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }
    out
}

/// Transmission RPC.
#[derive(Debug)]
pub struct Transmission {
    http: reqwest::Client,
    endpoint: String,
    username: Option<String>,
    password: Option<String>,
    session_id: Mutex<Option<String>>,
}

impl Transmission {
    pub fn new(base: &str, username: Option<String>, password: Option<String>) -> Self {
        let b = base.trim_end_matches('/');
        let endpoint = if b.ends_with("/rpc") {
            b.to_string()
        } else {
            format!("{b}/transmission/rpc")
        };
        Self {
            http: http(),
            endpoint,
            username,
            password,
            session_id: Mutex::new(None),
        }
    }

    async fn call(&self, body: &Value) -> Result<Value> {
        let mut sid = self.session_id.lock().await;
        for _ in 0..3 {
            let mut req = self.http.post(&self.endpoint).json(body);
            if let Some(s) = sid.as_deref() {
                req = req.header("X-Transmission-Session-Id", s);
            }
            if let Some(u) = self.username.as_deref().filter(|u| !u.is_empty()) {
                req = req.basic_auth(u, self.password.as_deref());
            }
            let resp = req.send().await?;
            if resp.status().as_u16() == 409 {
                *sid = resp
                    .headers()
                    .get("X-Transmission-Session-Id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                continue;
            }
            let status = resp.status();
            if !status.is_success() {
                return Err(Error::Remote {
                    status: status.as_u16(),
                    body: resp.text().await.unwrap_or_default(),
                });
            }
            return Ok(resp.json().await?);
        }
        Err(Error::Remote {
            status: 409,
            body: "transmission session id handshake failed".into(),
        })
    }

    pub async fn info(&self, hashes: &[String]) -> Result<HashMap<String, TorrentInfo>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let v = self
            .call(&json!({
                "method": "torrent-get",
                "arguments": {
                    "ids": hashes,
                    "fields": ["hashString", "percentDone", "rateDownload", "eta",
                               "peersSendingToUs", "peersConnected", "status", "totalSize"],
                }
            }))
            .await?;
        Ok(parse_transmission(&v))
    }
}

/// Parse a Transmission `torrent-get` response.
pub fn parse_transmission(v: &Value) -> HashMap<String, TorrentInfo> {
    let mut out = HashMap::new();
    let list = v
        .pointer("/arguments/torrents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for t in list {
        let Some(hash) = t.get("hashString").and_then(Value::as_str) else {
            continue;
        };
        let hash = hash.to_ascii_lowercase();
        let status = t.get("status").and_then(Value::as_i64).unwrap_or(-1);
        let state = match status {
            0 => "stopped",
            1 | 2 => "checking",
            3 => "queued_download",
            4 => "downloading",
            5 => "queued_seed",
            6 => "seeding",
            _ => "unknown",
        };
        out.insert(
            hash.clone(),
            TorrentInfo {
                hash,
                progress: t.get("percentDone").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                speed_bps: t.get("rateDownload").and_then(Value::as_u64).unwrap_or(0),
                eta_seconds: t
                    .get("eta")
                    .and_then(Value::as_i64)
                    .filter(|e| *e >= 0)
                    .map(|e| e as u64),
                seeds: t
                    .get("peersSendingToUs")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                peers: t.get("peersConnected").and_then(Value::as_u64).unwrap_or(0) as u32,
                size_bytes: t.get("totalSize").and_then(Value::as_u64).unwrap_or(0),
                state: state.into(),
            },
        );
    }
    out
}

/// Session cookie of the qBittorrent WebUI: `SID=…` (≤ 4.x) or `QBT_SID_<port>=…` (5.x).
fn is_session_cookie(pair: &str) -> bool {
    let name = pair.split('=').next().unwrap_or("").trim();
    name == "SID" || name.starts_with("QBT_SID")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param},
    };

    #[test]
    fn qbit_parse() {
        let v = json!([
            {"hash":"ABCDEF","progress":0.42,"dlspeed":1500000,"eta":8640000,"num_seeds":12,"num_leechs":3,"state":"downloading","total_size":5000},
            {"hash":"123","progress":1.0,"dlspeed":0,"eta":60,"num_seeds":0,"num_leechs":0,"state":"uploading","size":10}
        ]);
        let m = parse_qbittorrent(&v);
        let a = &m["abcdef"];
        assert_eq!(a.speed_bps, 1_500_000);
        assert_eq!(a.eta_seconds, None);
        assert_eq!((a.seeds, a.peers), (12, 3));
        assert!((a.progress - 0.42).abs() < 1e-6);
        assert_eq!(m["123"].eta_seconds, Some(60));
        assert_eq!(m["123"].size_bytes, 10);
    }

    #[test]
    fn transmission_parse() {
        let v = json!({"result":"success","arguments":{"torrents":[
            {"hashString":"AbC","percentDone":0.5,"rateDownload":2048,"eta":-1,"peersSendingToUs":4,"peersConnected":9,"status":4,"totalSize":100}
        ]}});
        let m = parse_transmission(&v);
        let t = &m["abc"];
        assert_eq!(t.eta_seconds, None);
        assert_eq!(t.state, "downloading");
        assert_eq!((t.seeds, t.peers, t.speed_bps), (4, 9, 2048));
    }

    #[tokio::test]
    async fn qbit_login_cookie_and_info() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/auth/login"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "SID=xyz; HttpOnly; path=/")
                    .set_body_string("Ok."),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/torrents/info"))
            .and(query_param("hashes", "aa|bb"))
            .and(header("cookie", "SID=xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"hash":"AA","progress":0.1,"dlspeed":10,"eta":5,"num_seeds":1,"num_leechs":2,"state":"downloading","size":9}
            ])))
            .mount(&s)
            .await;
        let c = TorrentClient::from_config(&TorrentClientConfig {
            kind: TorrentKind::Qbittorrent,
            url: s.uri(),
            username: Some("u".into()),
            password: Some("p".into()),
        });
        let m = c.info(&["aa".into(), "bb".into()]).await.unwrap();
        assert_eq!(m["aa"].peers, 2);
        // second call reuses the cookie (login expected exactly once)
        c.info(&["aa".into(), "bb".into()]).await.unwrap();
    }

    /// qBittorrent 5.x: login answers 204 with no body and the cookie is `QBT_SID_<port>`.
    #[tokio::test]
    async fn qbit5_login_cookie_name_and_204() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/auth/login"))
            .respond_with(ResponseTemplate::new(204).insert_header(
                "set-cookie",
                "QBT_SID_8200=abc123; HttpOnly; SameSite=Strict; path=/",
            ))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/torrents/info"))
            .and(header("cookie", "QBT_SID_8200=abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"hash":"CC","progress":1.0,"dlspeed":0,"eta":8640000,"num_seeds":0,"num_leechs":0,"state":"stalledUP","size":9}
            ])))
            .mount(&s)
            .await;
        let c = TorrentClient::from_config(&TorrentClientConfig {
            kind: TorrentKind::Qbittorrent,
            url: s.uri(),
            username: Some("admin".into()),
            password: Some("p".into()),
        });
        let m = c.info(&["cc".into()]).await.unwrap();
        assert_eq!(m["cc"].state, "stalledUP");
    }

    #[test]
    fn session_cookie_names() {
        assert!(is_session_cookie("SID=x"));
        assert!(is_session_cookie("QBT_SID_8080=x"));
        assert!(!is_session_cookie("SIDEBAR=x"));
        assert!(!is_session_cookie("other=1"));
    }

    #[tokio::test]
    async fn transmission_session_dance() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transmission/rpc"))
            .and(header("x-transmission-session-id", "sess1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result":"success","arguments":{"torrents":[
                    {"hashString":"AA","percentDone":0.25,"rateDownload":7,"eta":30,"peersSendingToUs":1,"peersConnected":2,"status":4,"totalSize":50}]}
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/transmission/rpc"))
            .respond_with(
                ResponseTemplate::new(409).insert_header("X-Transmission-Session-Id", "sess1"),
            )
            .mount(&s)
            .await;
        let c = TorrentClient::from_config(&TorrentClientConfig {
            kind: TorrentKind::Transmission,
            url: s.uri(),
            username: None,
            password: None,
        });
        let m = c.info(&["aa".into()]).await.unwrap();
        assert_eq!(m["aa"].eta_seconds, Some(30));
    }
}

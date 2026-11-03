//! Loopback upstream proxy: ffmpeg/ffprobe subprocesses never see provider URLs.
//!
//! Callers register an upstream URL (+ user agent) and get back a loopback URL
//! `http://127.0.0.1:{port}/internal/upstream/{token}/f.{ext}`. The proxy fetches the real URL
//! through a caller-supplied, SSRF-guarded `reqwest::Client` (connect-time DNS checks and redirect
//! checks live in that client, see `reel_iptv::net::client`), streams the body back with `Range`
//! passthrough, and rewrites HLS playlists so every nested URI (segments, keys, maps, child
//! playlists) becomes a fresh token. Tokens are unguessable (128 bit), expire, and are revoked
//! with their owner (the playback session). The listener only binds 127.0.0.1.
//!
//! This also keeps provider credentials out of subprocess argv and `/proc`.

use crate::error::PlaybackError;
use crate::redact::redact_text;
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderName, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Policy hook: `Err(reason)` refuses a URL before it is fetched (e.g. `reel_iptv::net::check_url`).
pub type UrlCheck = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

const MAX_ENTRIES: usize = 50_000;
const MAX_PLAYLIST: usize = 4 * 1024 * 1024;
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);

struct Entry {
    url: String,
    ua: String,
    expires: Instant,
    owner: Option<String>,
}

struct Inner {
    client: reqwest::Client,
    check: UrlCheck,
    entries: Mutex<HashMap<String, Entry>>,
    port: Mutex<Option<u16>>,
}

#[derive(Clone)]
pub struct UpstreamProxy(Arc<Inner>);

/// A registered URL that is revoked when dropped (for one-shot subprocesses).
pub struct Lease {
    proxy: UpstreamProxy,
    token: String,
    url: String,
}

impl Lease {
    /// Loopback URL to hand to the subprocess.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.proxy.0.entries.lock().unwrap().remove(&self.token);
    }
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Safe cosmetic file name (keeps the extension: ffmpeg's HLS demuxer filters segment extensions).
fn local_name(url: &reqwest::Url) -> String {
    let ext = url
        .path()
        .rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".into());
    format!("f.{ext}")
}

impl UpstreamProxy {
    pub fn new(client: reqwest::Client, check: UrlCheck) -> Self {
        UpstreamProxy(Arc::new(Inner {
            client,
            check,
            entries: Mutex::new(HashMap::new()),
            port: Mutex::new(None),
        }))
    }

    /// Number of live tokens (diagnostics / tests).
    pub fn token_count(&self) -> usize {
        self.0.entries.lock().unwrap().len()
    }

    /// Start the loopback listener on first use; returns its port.
    pub fn ensure_started(&self) -> Result<u16, PlaybackError> {
        let mut port = self.0.port.lock().unwrap();
        if let Some(p) = *port {
            return Ok(p);
        }
        let l = std::net::TcpListener::bind("127.0.0.1:0")?;
        l.set_nonblocking(true)?;
        let p = l.local_addr()?.port();
        let l = tokio::net::TcpListener::from_std(l)?;
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        let app = Router::new()
            .route("/internal/upstream/{token}/{name}", get(serve))
            .route("/internal/upstream/{token}", get(serve_bare))
            .with_state(weak.clone());
        tokio::spawn(async move {
            let stop = async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if weak.strong_count() == 0 {
                        break;
                    }
                }
            };
            let _ = axum::serve(l, app).with_graceful_shutdown(stop).await;
        });
        *port = Some(p);
        Ok(p)
    }

    fn register_inner(
        &self,
        url: &str,
        ua: &str,
        ttl: Duration,
        owner: Option<&str>,
    ) -> Result<(String, String), PlaybackError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| PlaybackError::Invalid("invalid upstream URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(PlaybackError::Invalid(
                "only http(s) upstream URLs are supported".into(),
            ));
        }
        (self.0.check)(url).map_err(|e| PlaybackError::ffmpeg(format!("upstream refused: {e}")))?;
        let port = self.ensure_started()?;
        let token = new_token();
        let local = format!(
            "http://127.0.0.1:{port}/internal/upstream/{token}/{}",
            local_name(&parsed)
        );
        let mut map = self.0.entries.lock().unwrap();
        if map.len() >= MAX_ENTRIES / 2 {
            let now = Instant::now();
            map.retain(|_, e| e.expires > now);
        }
        if map.len() >= MAX_ENTRIES {
            return Err(PlaybackError::Busy);
        }
        map.insert(
            token.clone(),
            Entry {
                url: url.to_string(),
                ua: ua.to_string(),
                expires: Instant::now() + ttl,
                owner: owner.map(str::to_string),
            },
        );
        Ok((token, local))
    }

    /// Register `url`; the returned loopback URL is valid for `ttl` or until `revoke_owner(owner)`.
    pub fn register(
        &self,
        url: &str,
        ua: &str,
        ttl: Duration,
        owner: Option<&str>,
    ) -> Result<String, PlaybackError> {
        self.register_inner(url, ua, ttl, owner)
            .map(|(_, local)| local)
    }

    /// One-shot registration, revoked when the lease is dropped.
    pub fn lease(&self, url: &str, ua: &str, ttl: Duration) -> Result<Lease, PlaybackError> {
        let (token, local) = self.register_inner(url, ua, ttl, None)?;
        Ok(Lease {
            proxy: self.clone(),
            token,
            url: local,
        })
    }

    /// Drop every token belonging to `owner` (session end).
    pub fn revoke_owner(&self, owner: &str) {
        self.0
            .entries
            .lock()
            .unwrap()
            .retain(|_, e| e.owner.as_deref() != Some(owner));
    }
}

fn gone() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

async fn serve_bare(
    State(w): State<Weak<Inner>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve(State(w), Path((token, String::new())), headers).await
}

async fn serve(
    State(w): State<Weak<Inner>>,
    Path((token, _)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(inner) = w.upgrade() else {
        return gone();
    };
    // loopback only: a browser reaching us through a rebound hostname carries a foreign Host
    let host_ok = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("127.0.0.1:") || h == "127.0.0.1");
    if !host_ok {
        return gone();
    }
    let (url, ua, expires, owner) = {
        let map = inner.entries.lock().unwrap();
        match map.get(&token) {
            Some(e) if e.expires > Instant::now() => {
                (e.url.clone(), e.ua.clone(), e.expires, e.owner.clone())
            }
            _ => return gone(),
        }
    };
    if let Err(e) = (inner.check)(&url) {
        tracing::warn!(reason = %redact_text(&e), "upstream request refused by policy");
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut rb = inner.client.get(&url).header(header::USER_AGENT, &ua);
    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if r.starts_with("bytes=") && r.len() < 128 {
            rb = rb.header(header::RANGE, r);
        }
    }
    let resp = match tokio::time::timeout(HEADER_TIMEOUT, rb.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(error = %redact_text(&e.to_string()), "upstream fetch failed");
            return StatusCode::BAD_GATEWAY.into_response();
        }
        Err(_) => return StatusCode::GATEWAY_TIMEOUT.into_response(),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let final_url = resp.url().clone();
    let is_playlist = status.is_success()
        && (ctype.contains("mpegurl") || final_url.path().to_ascii_lowercase().ends_with(".m3u8"));
    if is_playlist {
        let mut body: Vec<u8> = Vec::new();
        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    body.extend_from_slice(&c);
                    if body.len() > MAX_PLAYLIST {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                }
                Ok(None) => break,
                Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
            }
        }
        let text = String::from_utf8_lossy(&body);
        let proxy = UpstreamProxy(inner.clone());
        let ttl = expires.saturating_duration_since(Instant::now());
        let rewritten = rewrite_playlist(&text, &final_url, &mut |abs| {
            proxy.register(abs, &ua, ttl, owner.as_deref()).ok()
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(rewritten))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }
    let mut out = Response::builder().status(status);
    for h in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(v) = resp
            .headers()
            .get(HeaderName::from_bytes(h.as_str().as_bytes()).unwrap_or(h.clone()))
        {
            out = out.header(h, v.as_bytes());
        }
    }
    out.body(Body::from_stream(resp.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Rewrite every URI of an HLS playlist through `register(absolute_url) -> local_url`.
/// URIs that cannot be proxied (non-http schemes, refused by policy) are dropped.
pub fn rewrite_playlist(
    text: &str,
    base: &reqwest::Url,
    register: &mut dyn FnMut(&str) -> Option<String>,
) -> String {
    let mut map_uri = |raw: &str| -> Option<String> {
        if raw.starts_with("data:") {
            return Some(raw.to_string());
        }
        let abs = base.join(raw).ok()?;
        if !matches!(abs.scheme(), "http" | "https") {
            return None;
        }
        register(abs.as_str())
    };
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix('#') {
            // tags may carry URI="…" attributes (keys, maps, renditions, I-frame playlists, parts)
            let mut l = String::from("#");
            let mut tail = rest;
            while let Some(i) = tail.find("URI=\"") {
                l.push_str(&tail[..i + 5]);
                let after = &tail[i + 5..];
                let Some(end) = after.find('"') else {
                    l.push_str(after);
                    tail = "";
                    break;
                };
                if let Some(local) = map_uri(&after[..end]) {
                    l.push_str(&local);
                }
                l.push('"');
                tail = &after[end + 1..];
            }
            l.push_str(tail);
            out.push_str(&l);
            out.push('\n');
        } else {
            match map_uri(line.trim()) {
                Some(local) => out.push_str(&local),
                None => out.push_str("#REEL-DROPPED-URI"),
            }
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_every_uri_kind_and_drops_foreign_schemes() {
        let base = reqwest::Url::parse("https://cdn.example/path/master.m3u8?t=1").unwrap();
        let pl = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"keys/k1.key\",IV=0x1\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio/a.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo/v.m3u8\n#EXTINF:4,\nhttps://other.example/seg1.ts\nfile:///etc/passwd\n";
        let mut seen = Vec::new();
        let out = rewrite_playlist(pl, &base, &mut |abs| {
            seen.push(abs.to_string());
            Some(format!(
                "http://127.0.0.1:1/internal/upstream/T{}/f.x",
                seen.len()
            ))
        });
        assert_eq!(
            seen,
            [
                "https://cdn.example/path/keys/k1.key",
                "https://cdn.example/path/init.mp4",
                "https://cdn.example/path/audio/a.m3u8",
                "https://cdn.example/path/video/v.m3u8",
                "https://other.example/seg1.ts"
            ]
        );
        assert!(
            !out.contains("cdn.example")
                && !out.contains("other.example")
                && !out.contains("etc/passwd")
        );
        assert!(out.contains(
            "#EXT-X-KEY:METHOD=AES-128,URI=\"http://127.0.0.1:1/internal/upstream/T1/f.x\",IV=0x1"
        ));
        assert!(out.contains("#REEL-DROPPED-URI"));
    }

    #[test]
    fn local_names_keep_safe_extensions() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        assert_eq!(local_name(&u("http://h/a/b.M3U8?x=1")), "f.m3u8");
        assert_eq!(local_name(&u("http://h/a/b")), "f.bin");
        assert_eq!(local_name(&u("http://h/a/b.exe%00")), "f.bin");
    }
}

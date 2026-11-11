//! Loopback upstream proxy: ffmpeg/ffprobe subprocesses never see provider URLs.
//!
//! Callers register an upstream URL (+ user agent, optional referer) and get back a loopback URL
//! `http://127.0.0.1:{port}/internal/upstream/{token}/f.{ext}`. The proxy fetches the real URL
//! through a caller-supplied, SSRF-guarded `reqwest::Client` (connect-time DNS checks and redirect
//! checks live in that client, see `reel_iptv::net::client`), streams the body back with `Range`
//! passthrough, and rewrites HLS playlists so every nested URI (segments, keys, maps, child
//! playlists) becomes a token of its own. Tokens are unguessable (128 bit), expire, and are revoked
//! with their owner (the playback session, or the [`Lease`] they were derived from). The listener
//! only binds 127.0.0.1.
//!
//! Live HLS (the IPTV channel hub) uses [`UpstreamOpts::idle`]: tokens then expire a while after
//! their last use instead of at a fixed time, a reloaded playlist maps the same upstream URL to the
//! same token (so the token map does not grow with every reload), and a redirected playlist is
//! reloaded from where it was redirected to, like a player would.
//!
//! A playback session's input uses [`UpstreamOpts::exclusive`]: at most one provider connection is
//! open for its owner. ffmpeg's HTTP seek opens the new connection *before* closing the old one
//! (`http_seek_internal`: "if it fails, continue on old connection"), so without this every seek
//! (a matroska demuxer jump to the cues, a restart at a new position) briefly holds two provider
//! connections, which a one-connection provider refuses (HTTP 46x).
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
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Policy hook: `Err(reason)` refuses a URL before it is fetched (e.g. `reel_iptv::net::check_url`).
pub type UrlCheck = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

const MAX_ENTRIES: usize = 50_000;
const MAX_PLAYLIST: usize = 4 * 1024 * 1024;
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long an exclusive request waits for the previous connection of its owner to be released.
const EXCLUSIVE_WAIT: Duration = Duration::from_secs(10);
/// After cutting the previous connection, give its socket time to close (the FIN is sent by
/// hyper's connection task, asynchronously) before the provider sees the next request.
const EXCLUSIVE_SETTLE: Duration = Duration::from_millis(100);

/// How a registered URL is fetched and how long its token lives.
#[derive(Debug, Clone, Default)]
pub struct UpstreamOpts {
    pub user_agent: String,
    /// Sent as `Referer` on every upstream request (the URL itself and everything nested in it).
    pub referer: Option<String>,
    /// Sliding expiry for live streams: every request through a token keeps it (and the tokens
    /// derived from it) alive for this long. `None`: fixed expiry.
    pub idle: Option<Duration>,
    /// At most one open upstream response per owner: a new request through any token of the same
    /// owner first cuts the previous one and waits until it is released (newest wins). For a
    /// single-reader input (a session's media file); never for HLS (renditions load in parallel),
    /// so tokens derived from a playlist do not inherit it.
    pub exclusive: bool,
}

impl UpstreamOpts {
    pub fn ua(user_agent: &str) -> Self {
        UpstreamOpts {
            user_agent: user_agent.to_string(),
            ..Default::default()
        }
    }
}

struct Entry {
    url: String,
    opts: UpstreamOpts,
    expires: Instant,
    owner: Option<String>,
}

struct Inner {
    client: reqwest::Client,
    check: UrlCheck,
    /// Keys the deterministic tokens of nested playlist URIs.
    secret: String,
    entries: Mutex<HashMap<String, Entry>>,
    /// Exclusive owners: the one connection they may hold.
    slots: Mutex<HashMap<String, Slot>>,
    port: Mutex<Option<u16>>,
}

/// The connection slot of an exclusive owner.
struct Slot {
    /// Bumped by every request; the newest request owns the slot.
    seq: u64,
    /// Dropping it tells the current holder to let go.
    stop: tokio::sync::watch::Sender<()>,
    /// One permit, held for as long as an upstream response is open.
    gate: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub struct UpstreamProxy(Arc<Inner>);

/// A registered URL that is revoked when dropped (for one-shot subprocesses and live upstreams),
/// together with every token derived from it (segments, keys, child playlists).
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
        let t = &self.token;
        self.proxy
            .0
            .entries
            .lock()
            .unwrap()
            .retain(|k, e| k != t && e.owner.as_deref() != Some(t.as_str()));
        self.proxy.0.slots.lock().unwrap().remove(t);
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
            secret: new_token(),
            entries: Mutex::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
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

    /// Token of a nested URI: stable per (owner, URL), so a live playlist that is reloaded every
    /// few seconds maps its segments onto the same tokens instead of minting new ones.
    fn child_token(&self, owner: &str, url: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.0.secret.as_bytes());
        h.update([0]);
        h.update(owner.as_bytes());
        h.update([0]);
        h.update(url.as_bytes());
        h.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn register_inner(
        &self,
        url: &str,
        opts: &UpstreamOpts,
        expires: Instant,
        owner: Option<&str>,
        token: Option<String>,
        parent: Option<&str>,
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
        let token = token.unwrap_or_else(new_token);
        let local = format!(
            "http://127.0.0.1:{port}/internal/upstream/{token}/{}",
            local_name(&parsed)
        );
        let mut map = self.0.entries.lock().unwrap();
        // a nested URI only lives as long as the token it came from: checked under the same lock
        // as `revoke_owner` / lease drop, so a playlist that was still being fetched when its
        // owner ended cannot register tokens nothing would revoke
        if parent.is_some_and(|p| !map.contains_key(p)) {
            return Err(PlaybackError::Invalid("upstream owner ended".into()));
        }
        if let Some(e) = map.get_mut(&token) {
            // same URI seen again (playlist reload): keep the entry, extend its life
            e.expires = e.expires.max(expires);
            return Ok((token, local));
        }
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
                opts: opts.clone(),
                expires,
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
        self.register_inner(
            url,
            &UpstreamOpts::ua(ua),
            Instant::now() + ttl,
            owner,
            None,
            None,
        )
        .map(|(_, local)| local)
    }

    /// One-shot registration, revoked (with everything derived from it) when the lease is dropped.
    pub fn lease(&self, url: &str, ua: &str, ttl: Duration) -> Result<Lease, PlaybackError> {
        self.lease_with(url, &UpstreamOpts::ua(ua), ttl)
    }

    /// Like [`UpstreamProxy::lease`] with a referer and/or a sliding expiry
    /// ([`UpstreamOpts::idle`]; `ttl` then only bounds the time until the first request).
    pub fn lease_with(
        &self,
        url: &str,
        opts: &UpstreamOpts,
        ttl: Duration,
    ) -> Result<Lease, PlaybackError> {
        let token = new_token();
        let (token, local) = self.register_inner(
            url,
            opts,
            Instant::now() + ttl,
            Some(&token),
            Some(token.clone()),
            None,
        )?;
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
        // dropping the slot's stop sender also cuts a connection that is still open
        self.0.slots.lock().unwrap().remove(owner);
    }

    /// Register a URL with explicit [`UpstreamOpts`] (e.g. `exclusive` for a session's input);
    /// valid for `ttl` or until `revoke_owner(owner)`.
    pub fn register_with(
        &self,
        url: &str,
        opts: &UpstreamOpts,
        ttl: Duration,
        owner: Option<&str>,
    ) -> Result<String, PlaybackError> {
        self.register_inner(url, opts, Instant::now() + ttl, owner, None, None)
            .map(|(_, local)| local)
    }

    /// Exclusive requests: take the owner's slot (cutting the previous holder) and wait until its
    /// connection is released. `None`: superseded by a newer request, or the wait timed out.
    async fn claim_slot(
        &self,
        key: &str,
    ) -> Option<(
        u64,
        tokio::sync::watch::Receiver<()>,
        tokio::sync::OwnedSemaphorePermit,
    )> {
        let (seq, mut stop, gate) = {
            let mut slots = self.0.slots.lock().unwrap();
            let slot = slots.entry(key.to_string()).or_insert_with(|| Slot {
                seq: 0,
                stop: tokio::sync::watch::channel(()).0,
                gate: Arc::new(tokio::sync::Semaphore::new(1)),
            });
            slot.seq += 1;
            let (tx, rx) = tokio::sync::watch::channel(());
            // dropping the previous sender cuts the previous holder
            drop(std::mem::replace(&mut slot.stop, tx));
            (slot.seq, rx, slot.gate.clone())
        };
        let permit = match gate.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                let p = tokio::select! {
                    p = tokio::time::timeout(EXCLUSIVE_WAIT, gate.acquire_owned()) => p.ok()?.ok()?,
                    _ = stop.changed() => return None,
                };
                tokio::select! {
                    _ = tokio::time::sleep(EXCLUSIVE_SETTLE) => {}
                    _ = stop.changed() => return None,
                }
                p
            }
        };
        Some((seq, stop, permit))
    }

    /// Forget the owner's slot once its newest request is done.
    fn release_slot(&self, key: &str, seq: u64) {
        let mut slots = self.0.slots.lock().unwrap();
        if slots.get(key).is_some_and(|s| s.seq == seq) {
            slots.remove(key);
        }
    }
}

/// Copy an upstream body into `tx` until it ends, the client goes away or `stop` fires; the
/// permit (the owner's connection slot) is released together with the upstream response.
async fn pump_exclusive(
    resp: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<bytes::Bytes, reqwest::Error>>,
    mut stop: tokio::sync::watch::Receiver<()>,
    permit: tokio::sync::OwnedSemaphorePermit,
    proxy: UpstreamProxy,
    key: String,
    seq: u64,
) {
    use futures_util::StreamExt;
    let mut up = Box::pin(resp.bytes_stream());
    loop {
        let chunk = tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = tx.closed() => break,
            c = up.next() => match c {
                Some(c) => c,
                None => break,
            },
        };
        let failed = chunk.is_err();
        let sent = tokio::select! {
            biased;
            _ = stop.changed() => false,
            r = tx.send(chunk) => r.is_ok() && !failed,
        };
        if !sent {
            break;
        }
    }
    drop(up); // closes the provider connection …
    drop(permit); // … before the next request of this owner may open one
    proxy.release_slot(&key, seq);
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
    let (url, opts, expires, owner) = {
        let mut map = inner.entries.lock().unwrap();
        let now = Instant::now();
        match map.get_mut(&token) {
            Some(e) if e.expires > now => {
                if let Some(idle) = e.opts.idle {
                    e.expires = e.expires.max(now + idle);
                }
                (e.url.clone(), e.opts.clone(), e.expires, e.owner.clone())
            }
            _ => return gone(),
        }
    };
    if let Err(e) = (inner.check)(&url) {
        tracing::warn!(reason = %redact_text(&e), "upstream request refused by policy");
        return StatusCode::FORBIDDEN.into_response();
    }
    let proxy = UpstreamProxy(inner.clone());
    // exclusive owners: the previous connection is closed before this one is opened
    let slot_key = owner.clone().unwrap_or_else(|| token.clone());
    let mut slot = None;
    if opts.exclusive {
        match proxy.claim_slot(&slot_key).await {
            Some(s) => slot = Some(s),
            None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    let mut rb = inner
        .client
        .get(&url)
        .header(header::USER_AGENT, &opts.user_agent);
    if let Some(r) = &opts.referer {
        rb = rb.header(header::REFERER, r);
    }
    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if r.starts_with("bytes=") && r.len() < 128 {
            rb = rb.header(header::RANGE, r);
        }
    }
    let superseded = async {
        match slot.as_mut() {
            Some((_, stop, _)) => {
                let _ = stop.changed().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let resp = tokio::select! {
        r = tokio::time::timeout(HEADER_TIMEOUT, rb.send()) => r,
        _ = superseded => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let resp = match resp {
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
        // live playlists are reloaded from where they were redirected to, like a player does
        // (re-requesting the original URL can open a new provider session on every reload)
        if opts.idle.is_some()
            && final_url.as_str() != url
            && (inner.check)(final_url.as_str()).is_ok()
        {
            if let Some(e) = inner.entries.lock().unwrap().get_mut(&token) {
                e.url = final_url.to_string();
            }
        }
        drop(slot); // the playlist is read: its connection is done
        let text = String::from_utf8_lossy(&body);
        // renditions/segments load in parallel: nested tokens are never exclusive
        let opts = UpstreamOpts {
            exclusive: false,
            ..opts
        };
        let child_expires = match opts.idle {
            Some(idle) => Instant::now() + idle,
            None => expires,
        };
        let rewritten = rewrite_playlist(&text, &final_url, &mut |abs| {
            let tok = owner.as_deref().map(|o| proxy.child_token(o, abs));
            proxy
                .register_inner(
                    abs,
                    &opts,
                    child_expires,
                    owner.as_deref(),
                    tok,
                    Some(&token),
                )
                .ok()
                .map(|(_, local)| local)
        });
        if !inner.entries.lock().unwrap().contains_key(&token) {
            return gone(); // revoked while the playlist was in flight
        }
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
    let body = match slot {
        None => Body::from_stream(resp.bytes_stream()),
        Some((seq, stop, permit)) => {
            // a task owns the upstream so a newer request can cut it even while ffmpeg is not
            // reading (the old connection of an HTTP seek is left idle until the new one is open)
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(pump_exclusive(resp, tx, stop, permit, proxy, slot_key, seq));
            Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|c| (c, rx))
            }))
        }
    };
    out.body(body)
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

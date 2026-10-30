#![allow(dead_code)]
//! Test helpers: ffmpeg-generated MPEG-TS fixture and a simulated IPTV provider.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use reel_iptv::{Candidate, ChannelResolver, ChannelSpec, Result};

pub fn have_ffmpeg() -> bool {
    let enc = Command::new("ffmpeg").arg("-encoders").output();
    match enc {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("libx264") && s.contains(" aac ")
        }
        Err(_) => false,
    }
}

/// `secs` seconds of 25 fps H.264 + AAC in MPEG-TS with a keyframe exactly every second.
pub fn make_ts(path: &Path, secs: u32) {
    make_ts_audio(path, secs, "aac");
}

/// As [`make_ts`] with a chosen audio codec (e.g. `mp2`).
pub fn make_ts_audio(path: &Path, secs: u32, acodec: &str) {
    make_ts_codecs(path, secs, "libx264", acodec);
}

/// MPEG-TS with the given video/audio codecs and a keyframe every second.
pub fn make_ts_codecs(path: &Path, secs: u32, vcodec: &str, acodec: &str) {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=128x96:rate=25:duration={secs}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={secs}"))
        .args([
            "-c:v", vcodec, "-g", "25", "-bf", "0", "-pix_fmt", "yuv420p",
        ]);
    if vcodec == "libx264" {
        cmd.args([
            "-preset",
            "ultrafast",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
        ]);
    }
    let ok = cmd
        .args(["-c:a", acodec, "-b:a", "64k", "-f", "mpegts"])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg failed");
}

/// A VOD HLS rendition (`index.m3u8` + `.ts` segments) of `secs` seconds in `dir`.
pub fn make_hls(dir: &Path, secs: u32) {
    std::fs::create_dir_all(dir).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=128x96:rate=25:duration={secs}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={secs}"))
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-keyint_min",
            "25",
        ])
        .args([
            "-sc_threshold",
            "0",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .args([
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
        ])
        .arg(dir.join("seg%d.ts"))
        .arg(dir.join("index.m3u8"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg hls failed");
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Behavior {
    /// Stream the file in a loop.
    Normal,
    /// Stream this many bytes, then hang forever (stalled provider).
    StallAfter(usize),
    /// Answer with this HTTP status.
    Status(u16),
}

#[derive(Default)]
pub struct Counters {
    pub total: AtomicUsize,
    pub active: AtomicUsize,
    pub max_active: AtomicUsize,
}

pub struct Provider {
    pub addr: std::net::SocketAddr,
    behaviors: Arc<Mutex<HashMap<String, Behavior>>>,
    counters: Arc<Mutex<HashMap<String, Arc<Counters>>>>,
}

struct PState {
    data: Bytes,
    bytes_per_tick: usize,
    behaviors: Arc<Mutex<HashMap<String, Behavior>>>,
    counters: Arc<Mutex<HashMap<String, Arc<Counters>>>>,
}

struct ConnGuard(Arc<Counters>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, SeqCst);
    }
}

async fn serve_channel(State(st): State<Arc<PState>>, AxPath(name): AxPath<String>) -> Response {
    let c = st
        .counters
        .lock()
        .unwrap()
        .entry(name.clone())
        .or_default()
        .clone();
    c.total.fetch_add(1, SeqCst);
    let now = c.active.fetch_add(1, SeqCst) + 1;
    c.max_active.fetch_max(now, SeqCst);
    let guard = ConnGuard(c);
    let behavior = st
        .behaviors
        .lock()
        .unwrap()
        .get(&name)
        .copied()
        .unwrap_or(Behavior::Normal);
    if let Behavior::Status(code) = behavior {
        return StatusCode::from_u16(code).unwrap().into_response();
    }
    let stall_at = match behavior {
        Behavior::StallAfter(n) => Some(n),
        _ => None,
    };
    let (data, step) = (st.data.clone(), st.bytes_per_tick);
    let stream = futures_util::stream::unfold(
        (
            guard,
            0usize,
            0usize,
            tokio::time::interval(Duration::from_millis(20)),
        ),
        move |(guard, pos, sent, mut tick)| {
            let data = data.clone();
            async move {
                tick.tick().await;
                if stall_at.is_some_and(|n| sent >= n) {
                    futures_util::future::pending::<()>().await;
                }
                let n = step.min(data.len() - pos);
                let chunk = data.slice(pos..pos + n);
                let npos = (pos + n) % data.len();
                Some((
                    Ok::<_, std::io::Error>(chunk),
                    (guard, npos, sent + n, tick),
                ))
            }
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp2t")
        .body(Body::from_stream(stream))
        .unwrap()
}

impl Provider {
    /// Serve `file` in a loop at `speed`× its real-time bitrate; odd 20 ms chunk sizes on purpose.
    pub async fn start(file: &Path, secs: u32, speed: f64) -> Provider {
        let data = Bytes::from(std::fs::read(file).unwrap());
        let per_sec = data.len() as f64 / f64::from(secs) * speed;
        let bytes_per_tick = ((per_sec / 50.0) as usize).max(1) | 1; // odd → never 188-aligned
        let behaviors = Arc::new(Mutex::new(HashMap::new()));
        let counters = Arc::new(Mutex::new(HashMap::new()));
        let st = Arc::new(PState {
            data,
            bytes_per_tick,
            behaviors: behaviors.clone(),
            counters: counters.clone(),
        });
        let app = Router::new()
            .route("/live/{name}", get(serve_channel))
            .with_state(st);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Provider {
            addr,
            behaviors,
            counters,
        }
    }

    pub fn url(&self, name: &str) -> String {
        format!("http://{}/live/{name}", self.addr)
    }

    pub fn set(&self, name: &str, b: Behavior) {
        self.behaviors.lock().unwrap().insert(name.to_string(), b);
    }

    pub fn counters(&self, name: &str) -> Arc<Counters> {
        self.counters
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_default()
            .clone()
    }
}

#[derive(Default)]
pub struct MapResolver(pub Mutex<HashMap<i64, ChannelSpec>>);

impl MapResolver {
    pub fn add(&self, id: i64, name: &str, cands: Vec<Candidate>) {
        self.0.lock().unwrap().insert(
            id,
            ChannelSpec {
                channel_id: id,
                name: name.to_string(),
                candidates: cands,
            },
        );
    }
}

#[async_trait]
impl ChannelResolver for MapResolver {
    async fn resolve(&self, id: i64) -> Result<Option<ChannelSpec>> {
        Ok(self.0.lock().unwrap().get(&id).cloned())
    }
}

pub fn cand(channel_id: i64, source_id: i64, url: String, max: Option<u32>) -> Candidate {
    Candidate {
        channel_id,
        source_id,
        url,
        user_agent: None,
        referrer: None,
        max_connections: max,
    }
}

pub fn temp_ts(secs: u32) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("fixture.ts");
    make_ts(&p, secs);
    (dir, p)
}

/// Serve an HLS directory at `/` (`/index.m3u8`, `/segN.ts`) and the same playlist at
/// `/playlist` (no `.m3u8` extension, so only the Content-Type identifies it).
pub async fn serve_hls(dir: PathBuf) -> std::net::SocketAddr {
    use axum::extract::Path as AxPath;
    let d1 = dir.clone();
    let app = Router::new()
        .route(
            "/{file}",
            get(move |AxPath(file): AxPath<String>| {
                let dir = d1.clone();
                async move {
                    if file.contains('/') || file.contains("..") {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    match std::fs::read(dir.join(&file)) {
                        Ok(b) => {
                            let ct = if file.ends_with(".m3u8") {
                                "application/vnd.apple.mpegurl"
                            } else {
                                "video/mp2t"
                            };
                            ([(header::CONTENT_TYPE, ct)], b).into_response()
                        }
                        Err(_) => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        )
        .route(
            "/playlist",
            get(move || {
                let dir = dir.clone();
                async move {
                    let b = std::fs::read(dir.join("index.m3u8")).unwrap();
                    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")], b)
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

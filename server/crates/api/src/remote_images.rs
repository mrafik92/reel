//! Disk cache for third-party images: IPTV channel logos and VOD posters/stills (#9).
//!
//! The URLs come from the provider's playlist and catalog, so the bytes behind them are
//! untrusted. Downloads are capped in size and time while streaming, concurrent requests for one
//! URL share a single download, failures are remembered for a while, only real images are kept,
//! and the cache directories stay under a disk budget (least recently used files go first).
//! The SSRF policy is applied by the caller's HTTP client.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use reel_metadata::fetch::{
    DiskBudget, FAIL_TTL_PERMANENT, FAIL_TTL_TRANSIENT, FailCache, FetchError, SingleFlight,
    read_capped, touch, write_atomic,
};
use sha2::{Digest, Sha256};

/// Largest logo / VOD image accepted.
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;
/// Wall-clock limit for one download, body included.
pub const DEADLINE: Duration = Duration::from_secs(10);
/// Most files kept across the IPTV image caches.
pub const MAX_FILES: u64 = 100_000;
/// Default disk budget (`iptv_image_cache_max_mb`).
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024;
/// Cache subdirectories under `<data_dir>/iptv` that share the budget.
pub const DIRS: &[&str] = &["logos", "vod-images"];

#[derive(Debug)]
pub struct RemoteImages {
    root: PathBuf,
    flights: SingleFlight,
    failures: FailCache,
    budget: DiskBudget,
    max_bytes: u64,
    deadline: Duration,
}

impl RemoteImages {
    /// `iptv_dir` is `<data_dir>/iptv`; `budget_bytes == 0` means unlimited.
    pub fn new(iptv_dir: &Path, budget_bytes: u64) -> Self {
        Self::with_budget(
            iptv_dir,
            DiskBudget::new(
                DIRS.iter().map(|d| iptv_dir.join(d)).collect(),
                budget_bytes,
                MAX_FILES,
            ),
        )
    }

    fn with_budget(iptv_dir: &Path, budget: DiskBudget) -> Self {
        Self {
            root: iptv_dir.to_path_buf(),
            flights: SingleFlight::default(),
            failures: FailCache::default(),
            budget,
            max_bytes: MAX_IMAGE_BYTES,
            deadline: DEADLINE,
        }
    }

    /// The cached image for `url` in `<iptv_dir>/<sub>`, downloading it once if needed.
    /// `None` when it can't be had (upstream error, too large, too slow, not an image).
    pub async fn get(
        &self,
        http: &reqwest::Client,
        url: &str,
        sub: &str,
    ) -> Option<(Vec<u8>, &'static str)> {
        debug_assert!(DIRS.contains(&sub));
        let key: String = Sha256::digest(url.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let file = self.root.join(sub).join(&key);
        if let Some(hit) = read_cached(&file).await {
            return Some(hit);
        }
        let _g = self.flights.lock(&key).await;
        // whoever held the key before us may have fetched it, or failed
        if let Some(hit) = read_cached(&file).await {
            return Some(hit);
        }
        if self.failures.get(&key).is_some() {
            return None;
        }
        match self.download(http, url).await {
            Ok((bytes, ct)) => {
                if write_atomic(&file, &bytes).await.is_ok() {
                    self.budget.added(bytes.len() as u64).await;
                }
                Some((bytes, ct))
            }
            Err(ttl) => {
                self.failures.put(&key, None, ttl);
                None
            }
        }
    }

    /// Download and check one image. The error is how long to remember the failure.
    async fn download(
        &self,
        http: &reqwest::Client,
        url: &str,
    ) -> Result<(Vec<u8>, &'static str), Duration> {
        let resp = http
            .get(url)
            .timeout(self.deadline)
            .send()
            .await
            .map_err(|_| FAIL_TTL_TRANSIENT)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(if status.is_client_error() {
                FAIL_TTL_PERMANENT
            } else {
                FAIL_TTL_TRANSIENT
            });
        }
        let bytes = read_capped(resp, self.max_bytes, self.deadline)
            .await
            .map_err(|e| match e {
                FetchError::TooLarge(_) => FAIL_TTL_PERMANENT,
                _ => FAIL_TTL_TRANSIENT,
            })?;
        let ct = sniff_image(&bytes).ok_or(FAIL_TTL_PERMANENT)?;
        Ok((bytes, ct))
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        self.flights.in_flight()
    }
}

async fn read_cached(file: &Path) -> Option<(Vec<u8>, &'static str)> {
    let b = tokio::fs::read(file).await.ok()?;
    let ct = sniff_image(&b)?;
    touch(file).await;
    Some((b, ct))
}

pub(crate) fn sniff_image(b: &[u8]) -> Option<&'static str> {
    match b {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Router, body::Body, extract::State, routing::get};

    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0123456789";

    /// Local upstream counting requests per path:
    /// `/ok/*` a small PNG after 150 ms, `/big` an endless chunked body, `/slow` a PNG that
    /// trickles in, `/html` a web page, `/404` not found.
    async fn upstream() -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/ok/{name}",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    PNG
                }),
            )
            .route(
                "/big",
                get(|| async {
                    let chunks = futures_util::stream::repeat_with(|| {
                        Ok::<_, std::io::Error>(vec![0x89u8; 64 * 1024])
                    });
                    Body::from_stream(chunks)
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    let chunks = futures_util::stream::unfold(0u32, |i| async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let b: &[u8] = if i == 0 { PNG } else { b"." };
                        Some((Ok::<_, std::io::Error>(b.to_vec()), i + 1))
                    });
                    Body::from_stream(chunks)
                }),
            )
            .route("/html", get(|| async { "<html>no</html>" }))
            .route(
                "/404",
                get(|| async { (axum::http::StatusCode::NOT_FOUND, "") }),
            )
            .layer(axum::middleware::from_fn_with_state(
                hits.clone(),
                |State(h): State<Arc<AtomicUsize>>,
                 req: axum::extract::Request,
                 next: axum::middleware::Next| async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    next.run(req).await
                },
            ));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (base, hits)
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_download() {
        let (base, hits) = upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let c = Arc::new(RemoteImages::new(dir.path(), DEFAULT_MAX_BYTES));
        let mut tasks = vec![];
        for _ in 0..20 {
            let (c, url) = (c.clone(), format!("{base}/ok/a.png"));
            tasks.push(tokio::spawn(
                async move { c.get(&http(), &url, "logos").await },
            ));
        }
        for t in tasks {
            let (b, ct) = t.await.unwrap().expect("image");
            assert_eq!((b.as_slice(), ct), (PNG, "image/png"));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "one upstream request");
        assert_eq!(c.in_flight(), 0);
        // later requests are served from disk
        assert!(
            c.get(&http(), &format!("{base}/ok/a.png"), "logos")
                .await
                .is_some()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn oversized_slow_and_bogus_answers_are_cut_off_and_remembered() {
        let (base, hits) = upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let mut c = RemoteImages::new(dir.path(), DEFAULT_MAX_BYTES);
        c.deadline = Duration::from_millis(600);
        let h = http();
        for path in ["big", "slow", "html", "404"] {
            let t = std::time::Instant::now();
            let url = format!("{base}/{path}");
            // ten concurrent requests, then one more: all answered by a single download
            let all = futures_util::future::join_all((0..10).map(|_| c.get(&h, &url, "logos")));
            assert!(all.await.iter().all(Option::is_none), "{path}");
            assert!(c.get(&http(), &url, "logos").await.is_none(), "{path}");
            assert!(
                t.elapsed() < Duration::from_secs(3),
                "{path} took {:?}",
                t.elapsed()
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 4, "one request per bad URL");
        let kept = std::fs::read_dir(dir.path().join("logos"))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(kept, 0, "nothing cached");
    }

    #[tokio::test]
    async fn cache_stays_under_budget() {
        let (base, _) = upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let budget = PNG.len() as u64 * 4;
        let c = RemoteImages::with_budget(
            dir.path(),
            DiskBudget::new(
                DIRS.iter().map(|d| dir.path().join(d)).collect(),
                budget,
                MAX_FILES,
            )
            .with_grace(Duration::ZERO),
        );
        for i in 0..12 {
            let sub = DIRS[i % 2];
            assert!(
                c.get(&http(), &format!("{base}/ok/{i}.png"), sub)
                    .await
                    .is_some()
            );
        }
        let used: u64 = DIRS
            .iter()
            .flat_map(|d| std::fs::read_dir(dir.path().join(d)).into_iter().flatten())
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(used <= budget, "{used} > {budget}");
    }
}

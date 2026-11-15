//! Building blocks for caching remote images safely (#9):
//!
//! - [`read_capped`] streams a response body and stops as soon as it passes a byte cap or a
//!   wall-clock deadline, so a huge or endless (slow-loris) upstream can't fill memory or hold a
//!   worker.
//! - [`SingleFlight`] gives one async lock per cache key, so concurrent requests for the same
//!   image collapse into one upstream fetch. Entries are removed when the last holder lets go:
//!   the map only ever holds keys that are in flight.
//! - [`FailCache`] remembers recent failures for a short time (bounded), so waiters behind a
//!   failed fetch, and the next requests, don't hit the upstream again right away.
//! - [`DiskBudget`] keeps cache directories under a byte and file-count budget by deleting the
//!   least recently used files (by mtime; hits refresh it).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use tokio::sync::OwnedMutexGuard;

/// Why a capped read failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("body is larger than {0} bytes")]
    TooLarge(u64),
    #[error("body not received within {0:?}")]
    Deadline(Duration),
    /// URL stripped: image URLs from playlists may carry a login.
    #[error("http error: {0}")]
    Http(reqwest::Error),
}

impl From<reqwest::Error> for FetchError {
    fn from(e: reqwest::Error) -> Self {
        FetchError::Http(e.without_url())
    }
}

/// Read a response body into memory, failing as soon as it exceeds `max` bytes (an announced
/// `Content-Length` over `max` fails before any byte is read) or when the whole body hasn't
/// arrived within `deadline`.
pub async fn read_capped(
    mut resp: reqwest::Response,
    max: u64,
    deadline: Duration,
) -> Result<Vec<u8>, FetchError> {
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(FetchError::TooLarge(max));
    }
    let read = async move {
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if (out.len() + chunk.len()) as u64 > max {
                return Err(FetchError::TooLarge(max));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    };
    tokio::time::timeout(deadline, read)
        .await
        .map_err(|_| FetchError::Deadline(deadline))?
}

/// Per-key async locks that disappear once nobody holds or waits for them.
#[derive(Debug, Default)]
pub struct SingleFlight {
    map: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

/// Held while working on a key; dropping it lets the next waiter in.
pub struct FlightGuard {
    key: String,
    map: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl SingleFlight {
    /// Wait until no one else works on `key`, then hold it.
    pub async fn lock(&self, key: &str) -> FlightGuard {
        let m = {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            map.entry(key.to_string()).or_default().clone()
        };
        let guard = m.lock_owned().await;
        FlightGuard {
            key: key.to_string(),
            map: self.map.clone(),
            guard: Some(guard),
        }
    }

    /// Keys currently held or waited for.
    pub fn in_flight(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        // Release the lock first, then drop the map entry if no waiter holds a clone of it. Both
        // happen under the map lock, so no new waiter can pick up the entry in between.
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        drop(self.guard.take());
        if map
            .get(&self.key)
            .is_some_and(|m| Arc::strong_count(m) == 1)
        {
            map.remove(&self.key);
        }
    }
}

/// A recent failure: the upstream status (when there was one) and when to try again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub status: Option<u16>,
    until: Instant,
}

/// Bounded memory of recent failures per key.
#[derive(Debug)]
pub struct FailCache {
    map: Mutex<HashMap<String, Failure>>,
    cap: usize,
}

impl Default for FailCache {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl FailCache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: Mutex::default(),
            cap: cap.max(1),
        }
    }

    /// The remembered failure for `key`, if it hasn't expired.
    pub fn get(&self, key: &str) -> Option<Failure> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some(f) if f.until > Instant::now() => Some(*f),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    /// Remember that `key` failed, for `ttl`.
    pub fn put(&self, key: &str, status: Option<u16>, ttl: Duration) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.cap && !map.contains_key(key) {
            map.retain(|_, f| f.until > now);
            if map.len() >= self.cap {
                // still full of live entries: forget them all rather than grow
                map.clear();
            }
        }
        map.insert(
            key.to_string(),
            Failure {
                status,
                until: now + ttl,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// How long a failure is remembered: long for answers that won't change soon (4xx, too large,
/// not an image), short for transient ones (timeouts, 5xx, network errors).
pub const FAIL_TTL_PERMANENT: Duration = Duration::from_secs(30 * 60);
pub const FAIL_TTL_TRANSIENT: Duration = Duration::from_secs(2 * 60);

/// Byte and file-count budget over one or more cache directories. Usage is counted as files are
/// added; the first check (and every time the count goes over) walks the directories, and when
/// they are over budget, deletes the least recently modified files until usage is at 90 %.
#[derive(Debug)]
pub struct DiskBudget {
    dirs: Vec<PathBuf>,
    max_bytes: u64,
    max_files: u64,
    /// Files modified more recently than this are never deleted (they may be in use).
    grace: Duration,
    /// Estimated usage; `u64::MAX` until the first walk.
    bytes: AtomicU64,
    files: AtomicU64,
    pruning: tokio::sync::Mutex<()>,
}

/// Result of a [`DiskBudget::enforce`] walk.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneStats {
    pub bytes: u64,
    pub files: u64,
    pub removed_files: u64,
    pub removed_bytes: u64,
}

impl DiskBudget {
    /// `max_bytes == 0` means no budget (nothing is ever deleted).
    pub fn new(dirs: Vec<PathBuf>, max_bytes: u64, max_files: u64) -> Self {
        Self {
            dirs,
            max_bytes,
            max_files: max_files.max(1),
            grace: Duration::from_secs(60),
            bytes: AtomicU64::new(u64::MAX),
            files: AtomicU64::new(0),
            pruning: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Count a newly written file of `size` bytes and prune if the budget is exceeded.
    pub async fn added(&self, size: u64) {
        if self.max_bytes == 0 {
            return;
        }
        let b = self.bytes.load(Ordering::Relaxed);
        if b != u64::MAX {
            self.bytes.fetch_add(size, Ordering::Relaxed);
            self.files.fetch_add(1, Ordering::Relaxed);
        }
        if self.over() {
            self.enforce().await;
        }
    }

    fn over(&self) -> bool {
        self.bytes.load(Ordering::Relaxed) > self.max_bytes
            || self.files.load(Ordering::Relaxed) > self.max_files
    }

    /// Walk the directories, delete the oldest files while over budget, and reset the counters
    /// to what is on disk. Concurrent calls don't pile up: one walk runs, the others return.
    pub async fn enforce(&self) -> PruneStats {
        let Ok(_g) = self.pruning.try_lock() else {
            return PruneStats::default();
        };
        if self.max_bytes == 0 {
            return PruneStats::default();
        }
        let (dirs, max_b, max_f, grace) = (
            self.dirs.clone(),
            self.max_bytes,
            self.max_files,
            self.grace,
        );
        let stats = tokio::task::spawn_blocking(move || prune_dirs(&dirs, max_b, max_f, grace))
            .await
            .unwrap_or_default();
        self.bytes.store(stats.bytes, Ordering::Relaxed);
        self.files.store(stats.files, Ordering::Relaxed);
        if stats.removed_files > 0 {
            tracing::info!(
                removed = stats.removed_files,
                removed_mib = stats.removed_bytes / (1024 * 1024),
                kept_mib = stats.bytes / (1024 * 1024),
                "image cache over budget: removed least recently used files"
            );
        }
        stats
    }
}

/// Refresh a cached file's mtime on a hit (at most once an hour), so pruning is LRU. Best effort.
pub async fn touch(path: &Path) {
    let p = path.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        let now = SystemTime::now();
        let old = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > Duration::from_secs(3600));
        if old {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
                let _ = f.set_modified(now);
            }
        }
    })
    .await;
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        // `DirEntry::metadata` does not follow symlinks: only real files and dirs count
        if m.is_dir() && depth < 8 {
            walk(&e.path(), depth + 1, out);
        } else if m.is_file() {
            out.push((
                m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                m.len(),
                e.path(),
            ));
        }
    }
}

fn prune_dirs(dirs: &[PathBuf], max_bytes: u64, max_files: u64, grace: Duration) -> PruneStats {
    let mut files = Vec::new();
    for d in dirs {
        walk(d, 0, &mut files);
    }
    let mut st = PruneStats {
        bytes: files.iter().map(|f| f.1).sum(),
        files: files.len() as u64,
        ..Default::default()
    };
    if st.bytes <= max_bytes && st.files <= max_files {
        return st;
    }
    // go down to 90 % so we don't walk again on the next write
    let (low_b, low_f) = (max_bytes / 10 * 9, max_files / 10 * 9);
    let now = SystemTime::now();
    files.sort_by_key(|f| f.0);
    for (mtime, size, path) in files {
        if st.bytes <= low_b && st.files <= low_f {
            break;
        }
        if now.duration_since(mtime).unwrap_or_default() < grace {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            st.bytes -= size;
            st.files -= 1;
            st.removed_bytes += size;
            st.removed_files += 1;
        }
    }
    st
}

/// Write `bytes` to `path` through a unique temp file + rename, so readers never see a partial
/// file and two writers never share a temp name.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(d) = path.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    if let Err(e) = tokio::fs::write(&tmp, bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    tokio::fs::rename(&tmp, path).await.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A raw HTTP server that sends headers, then `chunks` body chunks `every` apart.
    async fn dribble(len: Option<u64>, chunks: usize, size: usize, every: Duration) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let head = match len {
                        Some(n) => format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\n\r\n"),
                        None => "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string(),
                    };
                    if s.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for _ in 0..chunks {
                        if s.write_all(&vec![b'x'; size]).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(every).await;
                    }
                });
            }
        });
        format!("http://{addr}/")
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn announced_oversize_is_refused_before_reading() {
        let url = dribble(Some(10_000_000), 0, 0, Duration::ZERO).await;
        let r = http().get(url).send().await.unwrap();
        assert!(matches!(
            read_capped(r, 1024, Duration::from_secs(5)).await,
            Err(FetchError::TooLarge(1024))
        ));
    }

    #[tokio::test]
    async fn endless_body_is_cut_at_the_cap() {
        // no Content-Length, 64 KiB every ms "forever": stops right after 1 MiB
        let url = dribble(None, 100_000, 64 * 1024, Duration::from_millis(1)).await;
        let r = http().get(url).send().await.unwrap();
        let t = Instant::now();
        assert!(matches!(
            read_capped(r, 1024 * 1024, Duration::from_secs(30)).await,
            Err(FetchError::TooLarge(_))
        ));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn slow_body_is_cut_at_the_deadline() {
        // 10 bytes every 100 ms, small enough to pass the size cap: the deadline ends it
        let url = dribble(None, 1000, 10, Duration::from_millis(100)).await;
        let r = http().get(url).send().await.unwrap();
        let t = Instant::now();
        assert!(matches!(
            read_capped(r, 1024 * 1024, Duration::from_millis(500)).await,
            Err(FetchError::Deadline(_))
        ));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn body_within_limits_is_returned() {
        let url = dribble(Some(3000), 3, 1000, Duration::ZERO).await;
        let r = http().get(url).send().await.unwrap();
        let b = read_capped(r, 3000, Duration::from_secs(5)).await.unwrap();
        assert_eq!(b.len(), 3000);
    }

    #[tokio::test]
    async fn single_flight_serializes_and_forgets_keys() {
        let sf = Arc::new(SingleFlight::default());
        let running = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let mut tasks = vec![];
        for _ in 0..20 {
            let (sf, running, peak) = (sf.clone(), running.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                let _g = sf.lock("k").await;
                let n = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
                running.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(sf.in_flight(), 0, "lock map must not keep finished keys");
        // a cancelled waiter doesn't leak its entry either
        let g = sf.lock("a").await;
        let w = tokio::time::timeout(Duration::from_millis(20), sf.lock("a")).await;
        assert!(w.is_err());
        drop(g);
        assert_eq!(sf.in_flight(), 0);
    }

    #[test]
    fn fail_cache_expires_and_stays_bounded() {
        let c = FailCache::new(3);
        c.put("a", Some(404), Duration::from_secs(60));
        assert_eq!(c.get("a").unwrap().status, Some(404));
        c.put("gone", None, Duration::ZERO);
        assert!(c.get("gone").is_none());
        for i in 0..100 {
            c.put(&format!("k{i}"), None, Duration::from_secs(60));
        }
        assert!(c.len() <= 3);
    }

    fn put_file(p: &Path, size: usize, age_secs: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![0u8; size]).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
    }

    #[tokio::test]
    async fn disk_budget_removes_least_recently_used() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        // 10 files of 1000 bytes over two dirs; file i is (100 - i) seconds old
        for i in 0..10u64 {
            let dir = if i % 2 == 0 { &a } else { &b };
            put_file(&dir.join("x").join(format!("f{i}")), 1000, 100 - i);
        }
        let budget = DiskBudget::new(vec![a.clone(), b.clone()], 5000, 1000)
            .with_grace(Duration::from_secs(10));
        let st = budget.enforce().await;
        assert!(st.bytes <= 4500, "{st:?}");
        assert_eq!(st.removed_files, 6);
        // the oldest went first, the newest stayed
        assert!(!a.join("x/f0").exists() && !b.join("x/f1").exists());
        assert!(a.join("x/f8").exists() && b.join("x/f9").exists());

        // the count follows writes and prunes when it goes over again
        for i in 10..20u64 {
            put_file(&a.join(format!("n{i}")), 1000, 50);
            budget.added(1000).await;
        }
        let total: u64 = [&a, &b]
            .iter()
            .flat_map(|d| walk_all(d))
            .map(|(_, s, _)| s)
            .sum();
        assert!(total <= 5000, "on disk {total}");

        // a file count budget works the same way
        let c = DiskBudget::new(vec![a.clone(), b.clone()], 1 << 30, 3).with_grace(Duration::ZERO);
        let st = c.enforce().await;
        assert!(st.files <= 3, "{st:?}");

        // files inside the grace period are never removed
        let fresh = tempfile::tempdir().unwrap();
        put_file(&fresh.path().join("new"), 5000, 0);
        let g = DiskBudget::new(vec![fresh.path().into()], 100, 10);
        assert_eq!(g.enforce().await.removed_files, 0);
        assert!(fresh.path().join("new").exists());
    }

    fn walk_all(d: &Path) -> Vec<(SystemTime, u64, PathBuf)> {
        let mut v = vec![];
        walk(d, 0, &mut v);
        v
    }

    #[tokio::test]
    async fn touch_refreshes_old_mtime() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        put_file(&p, 10, 7200);
        touch(&p).await;
        let m = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert!(SystemTime::now().duration_since(m).unwrap() < Duration::from_secs(60));
    }

    #[tokio::test]
    async fn write_atomic_leaves_no_temp_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("img.jpg");
        write_atomic(&p, b"abc").await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"abc");
        let names: Vec<_> = std::fs::read_dir(d.path().join("sub"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }
}

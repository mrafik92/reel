//! Ingestion budgets: hard caps on what a playlist, guide or provider API answer may cost us.
//!
//! A source URL is admin-supplied, but the bytes behind it come from a third party (the IPTV
//! provider or a public guide host). Every download is bounded in bytes and wall-clock time, every
//! decompression in expanded size, every XML token/attribute/text node in length, and every list
//! in record count, so a broken or hostile upstream cannot exhaust memory, disk or a worker.
//!
//! The values are deliberately generous: the real provider has ~41k movies, ~15k series and a
//! guide of a few hundred MB uncompressed, and these limits sit several times above that.

use std::{
    io::{self, BufRead, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::StreamExt;

use crate::{Error, Result};

const MIB: u64 = 1024 * 1024;

// ── M3U playlists ──
/// Largest playlist download (or local file) accepted.
pub const PLAYLIST_MAX_BYTES: u64 = 512 * MIB;
/// Wall-clock limit for one playlist download, body included.
pub const PLAYLIST_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// Longest single playlist line (an `#EXTINF` with all attributes, or a URL).
pub const M3U_MAX_LINE: usize = 64 * 1024;
/// Most channels one source may deliver (M3U entries or Xtream live streams).
pub const MAX_CHANNELS: usize = 500_000;

// ── Xtream `player_api.php` ──
/// Largest JSON answer (the VOD and series lists are the big ones: tens of MB for ~41k movies).
pub const XTREAM_JSON_MAX_BYTES: u64 = 256 * MIB;
/// Wall-clock limit for one API call, body included.
pub const XTREAM_DEADLINE: Duration = Duration::from_secs(120);
/// Most movies or series in one catalog list.
pub const MAX_VOD_ITEMS: usize = 500_000;
/// Most categories in one category list.
pub const MAX_CATEGORIES: usize = 50_000;

// ── XMLTV guides ──
/// Largest guide download kept on disk (compressed or not).
pub const EPG_MAX_DOWNLOAD_BYTES: u64 = 1024 * MIB;
/// Wall-clock limit for one guide download, body included.
pub const EPG_DEADLINE: Duration = Duration::from_secs(20 * 60);
/// Most XML bytes read from one guide after gunzipping (gzip-bomb guard).
pub const EPG_MAX_EXPANDED_BYTES: u64 = 4096 * MIB;
/// Longest single XML event (a start tag with its attributes, a text node, a comment, CDATA).
pub const XML_MAX_TOKEN: u64 = MIB;
/// Longest attribute value used (longer ones are ignored, e.g. a huge `data:` icon).
pub const XML_MAX_ATTR: usize = 8 * 1024;
/// Longest text kept for a title / description / display name (longer is truncated).
pub const XML_MAX_TEXT: usize = 16 * 1024;
/// Most `<channel>` elements in one guide.
pub const EPG_MAX_CHANNELS: usize = 200_000;
/// Most `<programme>` elements in one guide (in the window or not).
pub const EPG_MAX_PROGRAMMES: usize = 20_000_000;

pub(crate) fn too_large(what: &str, max: u64) -> Error {
    Error::Parse(format!("{what} is larger than the {} MiB limit", max / MIB))
}

pub(crate) fn too_many(what: &str, max: usize) -> Error {
    Error::Parse(format!("{what}: more than the limit of {max}"))
}

/// Fails when a list is longer than its budget.
pub fn check_count(what: &str, n: usize, max: usize) -> Result<()> {
    if n > max {
        Err(too_many(what, max))
    } else {
        Ok(())
    }
}

/// Refuse up front when the server announces a body over `max`.
pub fn check_content_length(resp: &reqwest::Response, what: &str, max: u64) -> Result<()> {
    match resp.content_length() {
        Some(n) if n > max => Err(too_large(what, max)),
        _ => Ok(()),
    }
}

/// Stream a response body into memory, failing as soon as it exceeds `max` bytes.
pub async fn read_body(resp: reqwest::Response, what: &str, max: u64) -> Result<Vec<u8>> {
    check_content_length(&resp, what, max)?;
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(chunk) = s.next().await {
        let chunk = chunk?;
        if (out.len() + chunk.len()) as u64 > max {
            return Err(too_large(what, max));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// A `Read` that fails once more than `max` bytes went through it.
pub struct CappedRead<R> {
    inner: R,
    left: u64,
    max: u64,
    what: &'static str,
}

impl<R> CappedRead<R> {
    pub fn new(inner: R, max: u64, what: &'static str) -> Self {
        Self {
            inner,
            left: max,
            max,
            what,
        }
    }
}

impl<R: Read> Read for CappedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // read one byte past the budget so an input of exactly `max` bytes still passes
        let want = buf.len().min(self.left.saturating_add(1) as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n as u64 > self.left {
            return Err(io::Error::other(format!(
                "{} is larger than the {} MiB limit",
                self.what,
                self.max / MIB
            )));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// A `BufRead` that fails when one parser event grows past `max` bytes. The parser calls
/// [`TokenBudget::reset`] after every event; quick-xml buffers a whole event before returning it,
/// so without this a single endless text node or tag would be read into memory in full.
pub struct TokenBudgetRead<R> {
    inner: R,
    used: Arc<AtomicU64>,
    max: u64,
}

/// Handle to reset the per-event byte count of a [`TokenBudgetRead`].
#[derive(Clone)]
pub struct TokenBudget(Arc<AtomicU64>);

impl TokenBudget {
    pub fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

impl<R: BufRead> TokenBudgetRead<R> {
    pub fn new(inner: R, max: u64) -> (Self, TokenBudget) {
        let used = Arc::new(AtomicU64::new(0));
        (
            Self {
                inner,
                used: used.clone(),
                max,
            },
            TokenBudget(used),
        )
    }

    fn check(&self) -> io::Result<()> {
        if self.used.load(Ordering::Relaxed) > self.max {
            return Err(io::Error::other(format!(
                "an XML element or text node is larger than the {} KiB limit",
                self.max / 1024
            )));
        }
        Ok(())
    }
}

impl<R: BufRead> Read for TokenBudgetRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.read(buf)?;
        self.used.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for TokenBudgetRead<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.check()?;
        // never expose more than the budget left (+1 to detect the overflow): a reader that hands
        // out its whole buffer at once would otherwise let one event through unchecked
        let left = (self.max - self.used.load(Ordering::Relaxed)).saturating_add(1);
        let b = self.inner.fill_buf()?;
        let n = b.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        Ok(&b[..n])
    }

    fn consume(&mut self, amt: usize) {
        self.used.fetch_add(amt as u64, Ordering::Relaxed);
        self.inner.consume(amt);
    }
}

/// Truncate `s` to at most `max` bytes, on a char boundary.
pub fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
}

/// A temp file that is removed when dropped, so every early return / `?` cleans it up.
#[derive(Debug)]
pub struct TempFile(Option<PathBuf>);

impl TempFile {
    pub fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    pub fn path(&self) -> &Path {
        self.0.as_deref().expect("temp file already released")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn capped_read_allows_exactly_max_and_fails_above() {
        let mut ok = CappedRead::new(Cursor::new(vec![1u8; 100]), 100, "x");
        let mut v = vec![];
        ok.read_to_end(&mut v).unwrap();
        assert_eq!(v.len(), 100);
        let mut bad = CappedRead::new(Cursor::new(vec![1u8; 101]), 100, "the guide");
        let e = bad.read_to_end(&mut vec![]).unwrap_err();
        assert!(e.to_string().contains("the guide is larger"), "{e}");
    }

    #[test]
    fn token_budget_resets_per_event() {
        let data = vec![b'a'; 4096];
        let (mut r, budget) = TokenBudgetRead::new(Cursor::new(data), 1000);
        let mut chunk = [0u8; 600];
        r.read_exact(&mut chunk).unwrap();
        budget.reset();
        r.read_exact(&mut chunk).unwrap();
        r.read_exact(&mut chunk).unwrap(); // 1200 since the reset
        assert!(r.fill_buf().is_err());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let mut s = "ééé".to_string(); // 6 bytes
        truncate_utf8(&mut s, 3);
        assert_eq!(s, "é");
    }

    #[test]
    fn temp_file_is_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.tmp");
        std::fs::write(&p, b"1").unwrap();
        drop(TempFile::new(p.clone()));
        assert!(!p.exists());
    }

    #[test]
    fn counts() {
        assert!(check_count("channels", 3, 3).is_ok());
        assert!(check_count("channels", 4, 3).is_err());
    }

    #[tokio::test]
    async fn read_body_caps_announced_and_streamed_bodies() {
        use axum::{Router, body::Body, routing::get};
        let app = Router::new()
            .route("/fixed", get(|| async { vec![b'a'; 5000] }))
            .route(
                "/chunked",
                get(|| async {
                    let chunks = (0..10).map(|_| Ok::<_, std::io::Error>(vec![b'b'; 1000]));
                    Body::from_stream(futures_util::stream::iter(chunks))
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for path in ["fixed", "chunked"] {
            let get = || http.get(format!("{base}/{path}")).send();
            let e = read_body(get().await.unwrap(), "the answer", 4096)
                .await
                .unwrap_err();
            assert!(
                e.to_string().contains("the answer is larger"),
                "{path}: {e}"
            );
            let ok = read_body(get().await.unwrap(), "the answer", 20_000)
                .await
                .unwrap();
            assert!(ok.len() >= 5000, "{path}");
        }
    }
}

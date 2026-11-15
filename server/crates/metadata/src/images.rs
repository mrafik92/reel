//! On-disk image cache: TMDB originals downloaded once, resized variants on demand.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use image::{ImageEncoder, codecs::jpeg::JpegEncoder, imageops::FilterType};

use crate::{
    Error, Result,
    fetch::{
        DiskBudget, FAIL_TTL_PERMANENT, FAIL_TTL_TRANSIENT, FailCache, FetchError, SingleFlight,
        read_capped, touch, write_atomic,
    },
};

pub const DEFAULT_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";
/// Allowed output widths. Anything above the largest means "original".
pub const WIDTHS: &[u32] = &[92, 185, 342, 500, 780, 1280];
const JPEG_QUALITY: u8 = 85;
/// Largest TMDB original we download (the biggest real one is ~4 MB).
pub const MAX_REMOTE_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Wall-clock limit for one TMDB download, body included.
pub const REMOTE_IMAGE_DEADLINE: Duration = Duration::from_secs(30);
/// Default disk budget of the image cache (`image_cache_max_mb`).
pub const DEFAULT_CACHE_MAX_BYTES: u64 = 4096 * 1024 * 1024;
/// Most files kept in the image cache, whatever their size.
pub const CACHE_MAX_FILES: u64 = 200_000;
/// Decoder limits for resizing: no side over 16k px, no more than 512 MiB of pixels.
const MAX_DECODE_SIDE: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Poster,
    Backdrop,
    Profile,
    Still,
}

impl ImageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Backdrop => "backdrop",
            Self::Profile => "profile",
            Self::Still => "still",
        }
    }
}

/// Clamp a requested width to the allowlist: smallest allowed >= `w`;
/// `0` or beyond the largest → `None` (original).
pub fn clamp_width(w: u32) -> Option<u32> {
    if w == 0 {
        return None;
    }
    WIDTHS.iter().copied().find(|a| *a >= w)
}

/// Disk cache of TMDB images. Downloads are size- and time-capped, concurrent requests for one
/// file collapse into one download or resize, recent failures are remembered for a while, and
/// the directory is kept under a disk budget (least recently used files go first).
#[derive(Debug, Clone)]
pub struct ImageCache {
    root: PathBuf,
    base: String,
    http: reqwest::Client,
    flights: Arc<SingleFlight>,
    failures: Arc<FailCache>,
    budget: Arc<DiskBudget>,
    max_bytes: u64,
}

impl ImageCache {
    /// `data_dir` is the Reel data directory; files live under `data_dir/images`.
    pub fn new(data_dir: &Path) -> Self {
        Self::with_base_url(data_dir, DEFAULT_IMAGE_BASE)
    }

    pub fn with_base_url(data_dir: &Path, base: impl Into<String>) -> Self {
        let root = data_dir.join("images");
        Self {
            budget: Arc::new(DiskBudget::new(
                vec![root.clone()],
                DEFAULT_CACHE_MAX_BYTES,
                CACHE_MAX_FILES,
            )),
            root,
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(REMOTE_IMAGE_DEADLINE)
                .build()
                .expect("reqwest client"),
            flights: Arc::default(),
            failures: Arc::default(),
            max_bytes: MAX_REMOTE_IMAGE_BYTES,
        }
    }

    /// Disk budget for the whole cache (originals and resized copies); `0` = unlimited.
    pub fn with_budget(mut self, max_bytes: u64, max_files: u64) -> Self {
        self.budget = Arc::new(DiskBudget::new(
            vec![self.root.clone()],
            max_bytes,
            max_files,
        ));
        self
    }

    /// Largest original accepted from TMDB.
    pub fn with_max_download(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The budget, for maintenance and tests.
    pub fn budget(&self) -> &DiskBudget {
        &self.budget
    }

    /// Keys being downloaded or resized right now.
    pub fn in_flight(&self) -> usize {
        self.flights.in_flight()
    }

    /// Return a cached file for a TMDB `source` (`/abc.jpg`) at `width`. Width is
    /// clamped to [`WIDTHS`]; `0`/large → the original file. `file://` sources are
    /// refused: use [`ImageCache::get_in`], which confines them to library roots.
    pub async fn get(&self, kind: ImageKind, source: &str, width: u32) -> Result<PathBuf> {
        self.get_in(kind, source, width, &[]).await
    }

    /// Like [`ImageCache::get`], but `file:///abs/path` local artwork is allowed when
    /// its fully resolved path (symlinks followed) lies under one of `roots` and the
    /// file starts with JPEG/PNG/WebP magic bytes. Anything else is `PermissionDenied`.
    pub async fn get_in(
        &self,
        kind: ImageKind,
        source: &str,
        width: u32,
        roots: &[PathBuf],
    ) -> Result<PathBuf> {
        let orig = self.original(kind, source, roots).await?;
        let Some(w) = clamp_width(width) else {
            return Ok(orig);
        };
        let name = file_key(source);
        let out = self
            .root
            .join(format!("w{w}"))
            .join(kind.as_str())
            .join(format!("{name}.jpg"));
        if out.exists() {
            touch(&out).await;
            return Ok(out);
        }
        let _g = self.flights.lock(&out.to_string_lossy()).await;
        if out.exists() {
            return Ok(out);
        }
        let (src, dst) = (orig.clone(), out.clone());
        let res = tokio::task::spawn_blocking(move || resize_to(&src, &dst, w))
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        match res? {
            Some(size) => {
                self.budget.added(size).await;
                Ok(out)
            }
            // Source already narrower than requested: serve original.
            None => Ok(orig),
        }
    }

    async fn original(&self, kind: ImageKind, source: &str, roots: &[PathBuf]) -> Result<PathBuf> {
        if let Some(p) = source.strip_prefix("file://") {
            return confine_local(Path::new(p), roots).await;
        }
        let path = self
            .root
            .join("orig")
            .join(kind.as_str())
            .join(file_key(source));
        if path.exists() {
            touch(&path).await;
            return Ok(path);
        }
        let key = path.to_string_lossy().into_owned();
        let _g = self.flights.lock(&key).await;
        // whoever held the key before us may have fetched it, or failed
        if path.exists() {
            return Ok(path);
        }
        if let Some(f) = self.failures.get(&key) {
            return Err(match f.status {
                Some(s) => Error::Status(s),
                None => Error::Rejected("image fetch failed recently".into()),
            });
        }
        match self.download(source).await {
            Ok(bytes) => {
                write_atomic(&path, &bytes).await?;
                self.budget.added(bytes.len() as u64).await;
                Ok(path)
            }
            Err(e) => {
                let (status, ttl) = match &e {
                    Error::Status(s) if (400..500).contains(s) => (Some(*s), FAIL_TTL_PERMANENT),
                    Error::Status(s) => (Some(*s), FAIL_TTL_TRANSIENT),
                    Error::Rejected(_) => (None, FAIL_TTL_PERMANENT),
                    _ => (None, FAIL_TTL_TRANSIENT),
                };
                self.failures.put(&key, status, ttl);
                Err(e)
            }
        }
    }

    async fn download(&self, source: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}", self.base, source.trim_start_matches('/'));
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Status(resp.status().as_u16()));
        }
        let bytes = read_capped(resp, self.max_bytes, REMOTE_IMAGE_DEADLINE)
            .await
            .map_err(|e| match e {
                FetchError::TooLarge(_) => Error::Rejected(e.to_string()),
                e => e.into(),
            })?;
        if !is_image_magic(&bytes) {
            return Err(Error::Rejected("not a JPEG/PNG/WebP file".into()));
        }
        Ok(bytes)
    }
}

/// Largest local artwork file we will serve (it is read into memory whole).
const MAX_LOCAL_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

fn denied(msg: &'static str) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        msg,
    ))
}

/// Resolve a local artwork path and make sure it is safe to serve: after following
/// every symlink it must be a regular file under one of `roots` (also resolved),
/// no larger than [`MAX_LOCAL_IMAGE_BYTES`], and start with image magic bytes.
/// Returns the resolved path.
async fn confine_local(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    use tokio::io::AsyncReadExt;

    let real = tokio::fs::canonicalize(path).await?;
    let mut inside = false;
    for r in roots {
        if let Ok(r) = tokio::fs::canonicalize(r).await {
            if real.starts_with(&r) {
                inside = true;
                break;
            }
        }
    }
    if !inside {
        return Err(denied("local image outside the library roots"));
    }
    let mut f = tokio::fs::File::open(&real).await?;
    let meta = f.metadata().await?;
    if !meta.is_file() || meta.len() > MAX_LOCAL_IMAGE_BYTES {
        return Err(denied("local image is not a regular file of sane size"));
    }
    let mut head = [0u8; 12];
    let n = f.read(&mut head).await?;
    if !is_image_magic(&head[..n]) {
        return Err(denied("local image is not a JPEG/PNG/WebP file"));
    }
    Ok(real)
}

fn is_image_magic(h: &[u8]) -> bool {
    h.starts_with(&[0xFF, 0xD8, 0xFF])
        || h.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || (h.len() >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP")
}

/// Filesystem-safe key for a source string.
fn file_key(source: &str) -> String {
    if let Some(p) = source.strip_prefix("file://") {
        // hash-ish: sanitized full path
        return sanitize(p);
    }
    sanitize(source.trim_start_matches('/'))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string()
}

/// Resize `src` to width `w` (Lanczos3, JPEG q85). Returns the written size, or `None`
/// (nothing written) when the source is not wider than `w`. Decoding is bounded by
/// [`MAX_DECODE_SIDE`] and [`MAX_DECODE_ALLOC`] (a small file can claim huge dimensions).
fn resize_to(src: &Path, dst: &Path, w: u32) -> Result<Option<u64>> {
    let mut reader = image::ImageReader::open(src)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let img = reader.decode()?;
    if img.width() <= w {
        return Ok(None);
    }
    let h = ((img.height() as f64) * (w as f64) / (img.width() as f64))
        .round()
        .max(1.0) as u32;
    let rgb = img.resize_exact(w, h, FilterType::Lanczos3).to_rgb8();
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut name = dst.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dst.with_file_name(name);
    {
        let f = std::fs::File::create(&tmp)?;
        JpegEncoder::new_with_quality(std::io::BufWriter::new(f), JPEG_QUALITY).write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;
    }
    let size = std::fs::metadata(&tmp)?.len();
    std::fs::rename(&tmp, dst)?;
    Ok(Some(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 90])
        });
        let mut buf = vec![];
        JpegEncoder::new_with_quality(&mut buf, 90)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    #[test]
    fn clamps() {
        assert_eq!(clamp_width(0), None);
        assert_eq!(clamp_width(100), Some(185));
        assert_eq!(clamp_width(342), Some(342));
        assert_eq!(clamp_width(1281), None);
    }

    #[tokio::test]
    async fn downloads_once_and_resizes() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/abc.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg_bytes(1000, 1500)))
            .expect(1)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let c = ImageCache::with_base_url(dir.path(), s.uri());
        let (a, b) = tokio::join!(
            c.get(ImageKind::Poster, "/abc.jpg", 300),
            c.get(ImageKind::Poster, "/abc.jpg", 300)
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a, b);
        assert!(a.to_string_lossy().contains("w342"));
        let img = image::open(&a).unwrap();
        assert_eq!((img.width(), img.height()), (342, 513));
        let orig = c.get(ImageKind::Poster, "/abc.jpg", 0).await.unwrap();
        assert!(orig.to_string_lossy().contains("orig"));
        // wider than source → original
        let big = c.get(ImageKind::Poster, "/abc.jpg", 1280).await.unwrap();
        assert_eq!(big, orig);
    }

    #[tokio::test]
    async fn concurrent_fetches_collapse_into_one() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(jpeg_bytes(400, 600))
                    .set_delay(Duration::from_millis(200)),
            )
            .expect(1)
            .mount(&s)
            .await;
        // a missing image: the failure is shared too, not retried by every waiter
        Mock::given(method("GET"))
            .and(path("/missing.jpg"))
            .respond_with(ResponseTemplate::new(404).set_delay(Duration::from_millis(100)))
            .expect(1)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let c = ImageCache::with_base_url(dir.path(), s.uri());
        let mut tasks = vec![];
        for i in 0..16u32 {
            let c = c.clone();
            tasks.push(tokio::spawn(async move {
                let w = [0, 185, 342][(i % 3) as usize];
                (
                    c.get(ImageKind::Poster, "/slow.jpg", w).await.is_ok(),
                    c.get(ImageKind::Poster, "/missing.jpg", w).await,
                )
            }));
        }
        for t in tasks {
            let (ok, missing) = t.await.unwrap();
            assert!(ok);
            assert!(matches!(missing, Err(Error::Status(404))));
        }
        assert_eq!(c.in_flight(), 0, "no lock entries left behind");
        // MockServer verifies `expect(1)` on drop
    }

    #[tokio::test]
    async fn oversized_and_non_image_originals_are_refused() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/huge.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xFF; 64 * 1024]))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/page.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
            .expect(1)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let c = ImageCache::with_base_url(dir.path(), s.uri()).with_max_download(16 * 1024);
        for p in ["/huge.jpg", "/page.jpg"] {
            // asked twice: the second answer comes from the failure memory
            for _ in 0..2 {
                assert!(matches!(
                    c.get(ImageKind::Poster, p, 0).await,
                    Err(Error::Rejected(_))
                ));
            }
        }
        let orig = dir.path().join("images").join("orig").join("poster");
        let left = std::fs::read_dir(&orig).map(|d| d.count()).unwrap_or(0);
        assert_eq!(left, 0, "nothing written for refused downloads");
    }

    #[tokio::test]
    async fn cache_stays_under_its_disk_budget() {
        let s = MockServer::start().await;
        let img = jpeg_bytes(300, 450);
        let size = img.len() as u64;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(img))
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let budget = size * 5;
        let c = ImageCache::with_base_url(dir.path(), s.uri()).with_budget(budget, 1000);
        // make the budget delete without waiting for the default grace period
        let c = ImageCache {
            budget: Arc::new(
                DiskBudget::new(vec![dir.path().join("images")], budget, 1000)
                    .with_grace(Duration::ZERO),
            ),
            ..c
        };
        for i in 0..20 {
            c.get(ImageKind::Poster, &format!("/p{i}.jpg"), 0)
                .await
                .unwrap();
        }
        let mut used = 0;
        let mut n = 0;
        for e in walkdir(&dir.path().join("images")) {
            used += std::fs::metadata(&e).unwrap().len();
            n += 1;
        }
        assert!(used <= budget, "cache uses {used} > budget {budget}");
        assert!((1..=5).contains(&n), "{n} files kept");
        // the newest download is still there
        assert!(dir.path().join("images/orig/poster/p19.jpg").exists());
    }

    fn walkdir(d: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn decoder_refuses_absurd_dimensions() {
        // a valid PNG header that claims 100000 x 100000 pixels
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("bomb.png");
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[0u8; 3], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        // patch width/height in the IHDR chunk (bytes 16..24) and fix its CRC
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        std::fs::write(&src, &png).unwrap();
        let r = resize_to(&src, &dir.path().join("out.jpg"), 185);
        assert!(
            matches!(r, Err(Error::Image(image::ImageError::Limits(_)))),
            "{r:?}"
        );
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    #[tokio::test]
    async fn local_files_resize() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let f = lib.join("poster.jpg");
        std::fs::write(&f, jpeg_bytes(600, 900)).unwrap();
        let c = ImageCache::with_base_url(dir.path(), "http://127.0.0.1:1");
        let out = c
            .get_in(
                ImageKind::Poster,
                &format!("file://{}", f.display()),
                185,
                &[lib.clone()],
            )
            .await
            .unwrap();
        assert_eq!(image::open(out).unwrap().width(), 185);
        // Original (w=0) is served too, as the resolved path.
        let orig = c
            .get_in(
                ImageKind::Poster,
                &format!("file://{}", f.display()),
                0,
                &[lib],
            )
            .await
            .unwrap();
        assert_eq!(orig, std::fs::canonicalize(&f).unwrap());
    }

    fn is_denied(r: Result<PathBuf>) -> bool {
        matches!(r, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
    }

    #[tokio::test]
    async fn local_art_is_confined_to_library_roots() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        let secret = dir.path().join("data");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("signing.key"), b"super secret key material").unwrap();
        std::fs::write(secret.join("other.jpg"), jpeg_bytes(50, 50)).unwrap();
        std::fs::write(lib.join("real.jpg"), jpeg_bytes(50, 50)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(secret.join("signing.key"), lib.join("poster.jpg")).unwrap();
            symlink(secret.join("other.jpg"), lib.join("folder.jpg")).unwrap();
            symlink(&lib, secret.join("alias")).unwrap();
        }
        let c = ImageCache::with_base_url(dir.path(), "http://127.0.0.1:1");
        let roots = [lib.clone()];
        let url = |p: &Path| format!("file://{}", p.display());

        // Plain file under the root works.
        assert!(
            c.get_in(ImageKind::Poster, &url(&lib.join("real.jpg")), 0, &roots)
                .await
                .is_ok()
        );
        #[cfg(unix)]
        {
            // Symlink to a non-image outside the roots: denied, for raw and resized.
            for w in [0, 185] {
                assert!(is_denied(
                    c.get_in(ImageKind::Poster, &url(&lib.join("poster.jpg")), w, &roots)
                        .await
                ));
            }
            // Symlink to a real image outside the roots: denied too.
            assert!(is_denied(
                c.get_in(ImageKind::Poster, &url(&lib.join("folder.jpg")), 0, &roots)
                    .await
            ));
            // `..` traversal out of the root: denied.
            let sneaky = format!("{}/../data/other.jpg", lib.display());
            assert!(is_denied(
                c.get_in(ImageKind::Poster, &format!("file://{sneaky}"), 0, &roots)
                    .await
            ));
        }
        // Absolute path elsewhere, and no roots at all: denied.
        assert!(is_denied(
            c.get_in(
                ImageKind::Poster,
                &url(&secret.join("other.jpg")),
                0,
                &roots
            )
            .await
        ));
        assert!(is_denied(
            c.get(ImageKind::Poster, &url(&lib.join("real.jpg")), 0)
                .await
        ));
    }

    #[tokio::test]
    async fn local_art_must_look_like_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let fake = lib.join("poster.jpg");
        std::fs::write(&fake, b"not an image at all, just text").unwrap();
        let c = ImageCache::with_base_url(dir.path(), "http://127.0.0.1:1");
        assert!(is_denied(
            c.get_in(
                ImageKind::Poster,
                &format!("file://{}", fake.display()),
                0,
                &[lib]
            )
            .await
        ));
    }

    #[test]
    fn magic_bytes() {
        assert!(is_image_magic(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]));
        assert!(is_image_magic(b"\x89PNG\r\n\x1a\n\0\0\0\0"));
        assert!(is_image_magic(b"RIFF\x00\x00\x00\x00WEBPVP8 "));
        assert!(!is_image_magic(b"RIFF\x00\x00\x00\x00WAVEfmt "));
        assert!(!is_image_magic(b"GIF89a"));
        assert!(!is_image_magic(b""));
    }
}

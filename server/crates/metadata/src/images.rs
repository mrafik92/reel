//! On-disk image cache: TMDB originals downloaded once, resized variants on demand.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use dashmap::DashMap;
use image::{ImageEncoder, codecs::jpeg::JpegEncoder, imageops::FilterType};
use tokio::sync::Mutex;

use crate::{Error, Result};

pub const DEFAULT_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";
/// Allowed output widths. Anything above the largest means "original".
pub const WIDTHS: &[u32] = &[92, 185, 342, 500, 780, 1280];
const JPEG_QUALITY: u8 = 85;

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

#[derive(Debug, Clone)]
pub struct ImageCache {
    root: PathBuf,
    base: String,
    http: reqwest::Client,
    locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
}

impl ImageCache {
    /// `data_dir` is the Reel data directory; files live under `data_dir/images`.
    pub fn new(data_dir: &Path) -> Self {
        Self::with_base_url(data_dir, DEFAULT_IMAGE_BASE)
    }

    pub fn with_base_url(data_dir: &Path, base: impl Into<String>) -> Self {
        Self {
            root: data_dir.join("images"),
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            locks: Arc::new(DashMap::new()),
        }
    }

    /// Return a cached file for `source` at `width`. `source` is a TMDB path
    /// (`/abc.jpg`) or a `file:///abs/path` local image. Width is clamped to
    /// [`WIDTHS`]; `0`/large → the original file.
    pub async fn get(&self, kind: ImageKind, source: &str, width: u32) -> Result<PathBuf> {
        let orig = self.original(kind, source).await?;
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
            return Ok(out);
        }
        let lock = self.lock_for(out.to_string_lossy().into_owned());
        let _g = lock.lock().await;
        if out.exists() {
            return Ok(out);
        }
        let (src, dst) = (orig.clone(), out.clone());
        let res = tokio::task::spawn_blocking(move || resize_to(&src, &dst, w))
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        match res? {
            true => Ok(out),
            // Source already narrower than requested: serve original.
            false => Ok(orig),
        }
    }

    fn lock_for(&self, key: String) -> Arc<Mutex<()>> {
        self.locks.entry(key).or_default().clone()
    }

    async fn original(&self, kind: ImageKind, source: &str) -> Result<PathBuf> {
        if let Some(p) = source.strip_prefix("file://") {
            let p = PathBuf::from(p);
            return if p.is_file() {
                Ok(p)
            } else {
                Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "local image missing",
                )))
            };
        }
        let path = self
            .root
            .join("orig")
            .join(kind.as_str())
            .join(file_key(source));
        if path.exists() {
            return Ok(path);
        }
        let lock = self.lock_for(path.to_string_lossy().into_owned());
        let _g = lock.lock().await;
        if path.exists() {
            return Ok(path);
        }
        let url = format!("{}/{}", self.base, source.trim_start_matches('/'));
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Status(resp.status().as_u16()));
        }
        let bytes = resp.bytes().await?;
        write_atomic(&path, &bytes).await?;
        Ok(path)
    }
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

async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Resize `src` to width `w` (Lanczos3, JPEG q85). Returns false (nothing
/// written) when the source is not wider than `w`.
fn resize_to(src: &Path, dst: &Path, w: u32) -> Result<bool> {
    let img = image::open(src)?;
    if img.width() <= w {
        return Ok(false);
    }
    let h = ((img.height() as f64) * (w as f64) / (img.width() as f64))
        .round()
        .max(1.0) as u32;
    let rgb = img.resize_exact(w, h, FilterType::Lanczos3).to_rgb8();
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = dst.with_extension("tmp");
    {
        let f = std::fs::File::create(&tmp)?;
        JpegEncoder::new_with_quality(std::io::BufWriter::new(f), JPEG_QUALITY).write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;
    }
    std::fs::rename(&tmp, dst)?;
    Ok(true)
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
    async fn local_files_resize() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("poster.jpg");
        std::fs::write(&f, jpeg_bytes(600, 900)).unwrap();
        let c = ImageCache::with_base_url(dir.path(), "http://127.0.0.1:1");
        let out = c
            .get(ImageKind::Poster, &format!("file://{}", f.display()), 185)
            .await
            .unwrap();
        assert_eq!(image::open(out).unwrap().width(), 185);
    }
}

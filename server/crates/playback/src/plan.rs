//! Segment plans: where each HLS segment starts (seconds, relative to file start).
//!
//! Encoded output uses a fixed grid (we force keyframes). Copy-mode output can only cut on source
//! keyframes, so the plan is derived from the real keyframe timestamps (probed once per file and
//! cached on disk) so the pre-generated VOD playlist matches what ffmpeg will really produce.

use crate::error::PlaybackError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentPlan {
    /// Start time of every segment; `starts[0] == 0`.
    starts: Vec<f64>,
    duration: f64,
    /// Copy-mode only: index (into the source keyframe list) of the keyframe starting each segment.
    #[serde(default)]
    kf_idx: Vec<u32>,
    /// Copy-mode only: total number of keyframes (GOPs) in the source.
    #[serde(default)]
    total_kfs: u32,
    /// Copy-mode only: time of every source keyframe (relative seconds).
    #[serde(default)]
    kf_times: Vec<f64>,
}

impl SegmentPlan {
    /// Fixed grid of `seg_secs`.
    pub fn fixed(duration: f64, seg_secs: u32) -> Self {
        let n = (duration / seg_secs as f64).ceil().max(0.0) as usize;
        SegmentPlan {
            starts: (0..n).map(|i| i as f64 * seg_secs as f64).collect(),
            duration,
            kf_idx: vec![],
            total_kfs: 0,
            kf_times: vec![],
        }
    }

    /// Greedy grouping of keyframes (relative seconds, sorted): a segment ends at the first
    /// keyframe at least `seg_secs` after the segment start.
    pub fn from_keyframes(keyframes: &[f64], duration: f64, seg_secs: u32) -> Self {
        let seg = seg_secs as f64;
        let mut starts = vec![0.0];
        let mut kf_idx = vec![0u32];
        let mut last = 0.0;
        let mut total = 0u32;
        for (i, &k) in keyframes.iter().enumerate() {
            if k >= duration - 0.05 {
                break;
            }
            total = i as u32 + 1;
            if i > 0 && k >= last + seg - 0.001 {
                starts.push(k);
                kf_idx.push(i as u32);
                last = k;
            }
        }
        SegmentPlan {
            starts,
            duration,
            kf_idx,
            total_kfs: total.max(1),
            kf_times: keyframes[..total as usize].to_vec(),
        }
    }

    /// True when segments are made of whole source GOPs (copy mode).
    pub fn is_keyframe_plan(&self) -> bool {
        !self.kf_idx.is_empty()
    }

    /// GOP files `[first, end)` that make up segment `i` (keyframe plans only).
    pub fn gop_range(&self, i: u32) -> (u32, u32) {
        let a = self.kf_idx.get(i as usize).copied().unwrap_or(0);
        let b = self
            .kf_idx
            .get(i as usize + 1)
            .copied()
            .unwrap_or(self.total_kfs);
        (a, b)
    }

    /// Time of source keyframe / GOP `g` (keyframe plans only).
    pub fn gop_time(&self, g: u32) -> Option<f64> {
        self.kf_times.get(g as usize).copied()
    }

    /// Index of the segment containing `t` seconds.
    pub fn segment_at(&self, t: f64) -> u32 {
        let i = self.starts.partition_point(|s| *s <= t + 0.001);
        (i.max(1) - 1) as u32
    }

    /// Index of the first GOP of segment `i` (keyframe plans only).
    pub fn first_gop(&self, i: u32) -> u32 {
        self.gop_range(i).0
    }

    pub fn len(&self) -> u32 {
        self.starts.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn start(&self, i: u32) -> f64 {
        self.starts
            .get(i as usize)
            .copied()
            .unwrap_or(self.duration)
    }

    /// Duration of segment `i`.
    pub fn seg_len(&self, i: u32) -> f64 {
        let end = self
            .starts
            .get(i as usize + 1)
            .copied()
            .unwrap_or(self.duration);
        (end - self.start(i)).max(0.0)
    }

    /// Start times of segments after `i` (cut points for ffmpeg).
    pub fn cuts_after(&self, i: u32) -> &[f64] {
        self.starts.get(i as usize + 1..).unwrap_or(&[])
    }

    pub fn max_seg_len(&self) -> f64 {
        (0..self.len()).map(|i| self.seg_len(i)).fold(0.0, f64::max)
    }
}

/// Parse `ffprobe -show_entries packet=pts_time,dts_time,flags -of csv=p=0` lines, returning
/// keyframe timestamps (sorted, deduplicated).
pub fn parse_keyframe_csv(out: &str) -> Vec<f64> {
    let mut v: Vec<f64> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split(',').collect();
            let flags = f.last()?;
            if !flags.contains('K') {
                return None;
            }
            f.iter()
                .take(f.len() - 1)
                .find_map(|x| x.parse::<f64>().ok())
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    v
}

#[derive(Serialize, Deserialize)]
struct KeyframeCache {
    keyframes: Vec<f64>,
}

fn cache_path(data_dir: &Path, file_id: i64, input: &str) -> Option<PathBuf> {
    if crate::ffmpeg::is_remote(input) {
        // remote (provider) file: keyed by a hash of its URL (never the URL itself, it carries
        // credentials); a replaced file with the same URL is caught by the duration check in
        // `cached_remote_keyframes`
        use sha2::{Digest, Sha256};
        let h: String = Sha256::digest(input.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return Some(
            data_dir
                .join("keyframes")
                .join(format!("{file_id}-remote-{h}.json")),
        );
    }
    let m = std::fs::metadata(input).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(
        data_dir
            .join("keyframes")
            .join(format!("{file_id}-{}-{mtime}.json", m.len())),
    )
}

async fn read_cache(c: &Path) -> Option<Vec<f64>> {
    let txt = tokio::fs::read_to_string(c).await.ok()?;
    serde_json::from_str::<KeyframeCache>(&txt)
        .ok()
        .map(|k| k.keyframes)
}

/// Keyframes cached on disk for this file, if any.
pub async fn cached_keyframes(data_dir: &Path, file_id: i64, input: &str) -> Option<Vec<f64>> {
    read_cache(&cache_path(data_dir, file_id, input)?).await
}

/// Persist a keyframe list in the per-file cache.
pub async fn store_keyframes(data_dir: &Path, file_id: i64, input: &str, kfs: &[f64]) {
    let Some(c) = cache_path(data_dir, file_id, input) else {
        return;
    };
    if let Some(parent) = c.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Ok(j) = serde_json::to_string(&KeyframeCache {
        keyframes: kfs.to_vec(),
    }) {
        let tmp = c.with_extension("json.tmp");
        if tokio::fs::write(&tmp, j).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &c).await;
        }
    }
}

/// An index is usable only if it is dense enough to cut segments from and covers the file.
pub fn index_is_usable(kfs: &[f64], duration: f64) -> bool {
    if kfs.len() < 2 {
        return false;
    }
    let max_gap = kfs.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    max_gap <= 30.0 && kfs.last().copied().unwrap_or(0.0) >= duration - 60.0
}

/// Read the keyframe index from container metadata only, bounded by `timeout`.
/// `None` when the container has no usable index or reading was too slow.
pub async fn fast_keyframes(input: &str, timeout: Duration, duration: f64) -> Option<Vec<f64>> {
    let path = input.to_string();
    let task = tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&path)?;
        let mut r = crate::keyindex::Limited::new(f, Some(timeout));
        crate::keyindex::read_keyframes(&mut r)
    });
    let res = tokio::time::timeout(timeout + Duration::from_millis(250), task)
        .await
        .ok()?
        .ok()?;
    match res {
        Ok(Some(k)) if index_is_usable(&k, duration) => Some(k),
        _ => None,
    }
}

/// Cached index of a remote file, if it still fits the file's (probed) duration.
pub async fn cached_remote_keyframes(
    data_dir: &Path,
    file_id: i64,
    url: &str,
    duration: f64,
) -> Option<Vec<f64>> {
    cached_keyframes(data_dir, file_id, url)
        .await
        .filter(|k| index_is_usable(k, duration))
}

/// Limits of [`remote_keyframes`]: a handful of ranged reads, never a download of the file.
#[derive(Debug, Clone, Copy)]
pub struct RemoteIndexLimits {
    pub timeout: Duration,
    /// Most HTTP requests (each a ranged GET, one at a time).
    pub max_requests: u32,
    /// Most bytes read in total.
    pub max_bytes: u64,
}

impl Default for RemoteIndexLimits {
    fn default() -> Self {
        RemoteIndexLimits {
            timeout: Duration::from_secs(8),
            max_requests: 12,
            max_bytes: 24 << 20,
        }
    }
}

/// Read the keyframe index (Matroska Cues / MP4 `stss`) of a *remote* file from its container
/// metadata with a few ranged GETs, one at a time. `url` must be the session's loopback
/// upstream-proxy URL (never a provider URL): the proxy adds the provider's User-Agent and keeps
/// the owner's single provider connection. `None` when the server ignores `Range`, the container
/// has no usable index, or a limit is hit.
pub async fn remote_keyframes(
    url: &str,
    limits: RemoteIndexLimits,
    duration: f64,
) -> Option<Vec<f64>> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let handle = tokio::runtime::Handle::current();
    let url = url.to_string();
    let task = tokio::task::spawn_blocking(move || {
        let r = RangeReader::open(client, url, handle, limits)?;
        let mut r = crate::keyindex::Limited::new(r, Some(limits.timeout));
        crate::keyindex::read_keyframes(&mut r)
    });
    let res = tokio::time::timeout(limits.timeout + Duration::from_millis(500), task)
        .await
        .ok()?
        .ok()?;
    match res {
        Ok(Some(k)) if index_is_usable(&k, duration) => Some(k),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(error = %e, "remote keyframe index not readable");
            None
        }
    }
}

/// Smallest ranged read: the container head (EBML header, SeekHead, Tracks / `ftyp`, `moov`
/// headers) and most Matroska Cues fit in one request.
const RANGE_BLOCK: u64 = 1 << 20;

/// `Read + Seek` over HTTP `Range` requests (blocking; runs on a `spawn_blocking` thread), with
/// every fetched range kept in memory so the index parsers' small reads cost no extra request.
struct RangeReader {
    client: reqwest::Client,
    url: String,
    rt: tokio::runtime::Handle,
    len: u64,
    pos: u64,
    blocks: Vec<(u64, Vec<u8>)>,
    requests: u32,
    bytes: u64,
    limits: RemoteIndexLimits,
}

fn io_err(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::other(msg.into())
}

impl RangeReader {
    fn open(
        client: reqwest::Client,
        url: String,
        rt: tokio::runtime::Handle,
        limits: RemoteIndexLimits,
    ) -> std::io::Result<Self> {
        let mut r = RangeReader {
            client,
            url,
            rt,
            len: u64::MAX,
            pos: 0,
            blocks: Vec::new(),
            requests: 0,
            bytes: 0,
            limits,
        };
        r.fetch(0, RANGE_BLOCK)?;
        Ok(r)
    }

    /// GET `bytes=start-(start+want-1)`; learns the total length from `Content-Range`.
    fn fetch(&mut self, start: u64, want: u64) -> std::io::Result<()> {
        if self.requests >= self.limits.max_requests {
            return Err(io_err("remote index: request limit"));
        }
        let want = want.min(self.len.saturating_sub(start)).max(1);
        if self.bytes + want > self.limits.max_bytes {
            return Err(io_err("remote index: byte limit"));
        }
        self.requests += 1;
        let end = start + want - 1;
        let (client, url) = (self.client.clone(), self.url.clone());
        let (total, data) = self.rt.block_on(async move {
            use futures_util::StreamExt;
            let resp = client
                .get(&url)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await
                .map_err(|e| io_err(format!("remote index: {}", e.without_url())))?;
            // a server that ignores Range would send the whole file: never read that
            if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(io_err(format!("remote index: HTTP {}", resp.status())));
            }
            let (from, total) = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .ok_or_else(|| io_err("remote index: no Content-Range"))?;
            if from != start {
                return Err(io_err("remote index: wrong range"));
            }
            let mut data = Vec::with_capacity(want as usize);
            let mut body = Box::pin(resp.bytes_stream());
            while let Some(c) = body.next().await {
                let c = c.map_err(|e| io_err(format!("remote index: {}", e.without_url())))?;
                if (data.len() + c.len()) as u64 > want {
                    return Err(io_err("remote index: response longer than asked"));
                }
                data.extend_from_slice(&c);
            }
            Ok::<_, std::io::Error>((total, data))
        })?;
        if data.is_empty() {
            return Err(io_err("remote index: empty range"));
        }
        self.bytes += data.len() as u64;
        self.len = total;
        self.blocks.push((start, data));
        Ok(())
    }
}

/// `bytes A-B/TOTAL` → (A, TOTAL).
fn parse_content_range(v: &str) -> Option<(u64, u64)> {
    let (range, total) = v.strip_prefix("bytes ")?.split_once('/')?;
    let (a, _) = range.split_once('-')?;
    Some((a.trim().parse().ok()?, total.trim().parse().ok()?))
}

impl std::io::Read for RangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let hit = |blocks: &[(u64, Vec<u8>)], pos: u64| {
            blocks
                .iter()
                .find(|(s, d)| pos >= *s && pos < s + d.len() as u64)
                .map(|(s, d)| (*s, d.len()))
        };
        if hit(&self.blocks, self.pos).is_none() {
            self.fetch(self.pos, (buf.len() as u64).max(RANGE_BLOCK))?;
        }
        let (s, _) = hit(&self.blocks, self.pos).ok_or_else(|| io_err("remote index: short"))?;
        let data = &self.blocks.iter().find(|(b, _)| *b == s).expect("hit").1;
        let off = (self.pos - s) as usize;
        let n = buf.len().min(data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for RangeReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let p = match pos {
            std::io::SeekFrom::Start(p) => Some(p),
            std::io::SeekFrom::End(d) => self.len.checked_add_signed(d),
            std::io::SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = p.ok_or_else(|| io_err("remote index: bad seek"))?;
        Ok(self.pos)
    }
}

/// Run ffprobe over every packet (reads the whole file!). `low_priority` wraps it in
/// `ionice -c3`/`nice -n 19` where available. Dropping the future kills the process.
pub async fn probe_keyframes_cmd(
    ffprobe: &str,
    input: &str,
    timeout: Duration,
    low_priority: bool,
) -> Result<Vec<f64>, PlaybackError> {
    let mut prefix: Vec<&str> = Vec::new();
    if low_priority {
        if which("ionice") {
            prefix.extend(["ionice", "-c3"]);
        }
        if which("nice") {
            prefix.extend(["nice", "-n", "19"]);
        }
    }
    let run = |args: Vec<&str>| {
        let mut full: Vec<&str> = prefix.clone();
        full.push(ffprobe);
        full.extend(args);
        let mut cmd = Command::new(full[0]);
        cmd.args(&full[1..])
            .arg(input)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        async move { tokio::time::timeout(timeout, cmd.output()).await }
    };
    let pk = run(vec![
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,dts_time,flags",
        "-of",
        "csv=p=0",
    ])
    .await
    .map_err(|_| PlaybackError::ffmpeg("keyframe probe timed out"))??;
    if !pk.status.success() {
        return Err(PlaybackError::ffmpeg(
            String::from_utf8_lossy(&pk.stderr).trim(),
        ));
    }
    let mut kfs = parse_keyframe_csv(&String::from_utf8_lossy(&pk.stdout));
    let st = run(vec![
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-show_entries",
        "format=start_time",
        "-of",
        "csv=p=0",
    ])
    .await
    .map_err(|_| PlaybackError::ffmpeg("probe timed out"))??;
    let start: f64 = String::from_utf8_lossy(&st.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0);
    for k in kfs.iter_mut() {
        *k = (*k - start).max(0.0);
    }
    Ok(kfs)
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Keyframe times (relative to the container start) of the first video stream, via the disk
/// cache or a full ffprobe scan (slow on big files; prefer [`fast_keyframes`] + background scan).
pub async fn probe_keyframes(
    ffprobe: &str,
    data_dir: &Path,
    file_id: i64,
    input: &str,
    timeout: Duration,
) -> Result<Vec<f64>, PlaybackError> {
    if let Some(k) = cached_keyframes(data_dir, file_id, input).await {
        return Ok(k);
    }
    let kfs = probe_keyframes_cmd(ffprobe, input, timeout, false).await?;
    store_keyframes(data_dir, file_id, input, &kfs).await;
    Ok(kfs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_plan_covers_duration() {
        let p = SegmentPlan::fixed(10.0, 4);
        assert_eq!(p.len(), 3);
        assert_eq!(p.seg_len(2), 2.0);
        assert_eq!((0..3).map(|i| p.seg_len(i)).sum::<f64>(), 10.0);
    }

    #[test]
    fn keyframe_plan_groups_irregular_gops() {
        // GOP of 3.7s
        let kfs: Vec<f64> = (0..8).map(|i| i as f64 * 3.7).collect();
        let p = SegmentPlan::from_keyframes(&kfs, 26.0, 4);
        // 0, 7.4 (first kf >= 4 is 7.4), 14.8, 22.2
        assert_eq!(p.starts, vec![0.0, kfs[2], kfs[4], kfs[6]]);
        assert_eq!(p.gop_range(1), (2, 4));
        assert_eq!(p.gop_range(3), (6, 8));
        assert!(p.is_keyframe_plan());
        assert_eq!(p.segment_at(0.0), 0);
        assert_eq!(p.segment_at(7.3), 0);
        assert_eq!(p.segment_at(7.4), 1);
        assert_eq!(p.segment_at(100.0), 3);
        assert_eq!(p.gop_time(2), Some(kfs[2]));
        assert!((0..p.len()).map(|i| p.seg_len(i)).sum::<f64>() - 26.0 < 1e-9);
        assert_eq!(p.cuts_after(1), &[kfs[4], kfs[6]]);
    }

    #[test]
    fn parses_ffprobe_csv() {
        let out = "0.000000,0.000000,K__\n0.040000,0.040000,___\n4.000000,N/A,K_\n2.0,1.0,K__\n";
        assert_eq!(parse_keyframe_csv(out), vec![0.0, 2.0, 4.0]);
    }
}

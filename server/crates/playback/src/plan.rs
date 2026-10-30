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
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,dts_time,flags",
        "-of",
        "csv=p=0",
    ])
    .await
    .map_err(|_| PlaybackError::Ffmpeg("keyframe probe timed out".into()))??;
    if !pk.status.success() {
        return Err(PlaybackError::Ffmpeg(
            String::from_utf8_lossy(&pk.stderr).trim().to_string(),
        ));
    }
    let mut kfs = parse_keyframe_csv(&String::from_utf8_lossy(&pk.stdout));
    let st = run(vec![
        "-v",
        "error",
        "-show_entries",
        "format=start_time",
        "-of",
        "csv=p=0",
    ])
    .await
    .map_err(|_| PlaybackError::Ffmpeg("probe timed out".into()))??;
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
        assert!((0..p.len()).map(|i| p.seg_len(i)).sum::<f64>() - 26.0 < 1e-9);
        assert_eq!(p.cuts_after(1), &[kfs[4], kfs[6]]);
    }

    #[test]
    fn parses_ffprobe_csv() {
        let out = "0.000000,0.000000,K__\n0.040000,0.040000,___\n4.000000,N/A,K_\n2.0,1.0,K__\n";
        assert_eq!(parse_keyframe_csv(out), vec![0.0, 2.0, 4.0]);
    }
}

//! Hardware acceleration detection (ffmpeg subprocess probing).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use utoipa::ToSchema;

pub const DEFAULT_VAAPI_DEVICE: &str = "/dev/dri/renderD128";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HwAccel {
    Vaapi { device: String },
    Qsv { device: String },
    Nvenc,
    VideoToolbox,
    Software,
}

impl HwAccel {
    pub fn name(&self) -> &'static str {
        match self {
            HwAccel::Vaapi { .. } => "vaapi",
            HwAccel::Qsv { .. } => "qsv",
            HwAccel::Nvenc => "nvenc",
            HwAccel::VideoToolbox => "videotoolbox",
            HwAccel::Software => "software",
        }
    }
    /// ffmpeg H.264 encoder name for this backend.
    pub fn h264_encoder(&self) -> &'static str {
        match self {
            HwAccel::Vaapi { .. } => "h264_vaapi",
            HwAccel::Qsv { .. } => "h264_qsv",
            HwAccel::Nvenc => "h264_nvenc",
            HwAccel::VideoToolbox => "h264_videotoolbox",
            HwAccel::Software => "libx264",
        }
    }
}

/// Result of startup probing, exposed in the admin "hw accel status" endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HwAccelStatus {
    pub selected: HwAccel,
    /// Backends that passed a 1-frame test encode, in preference order.
    pub available: Vec<HwAccel>,
    /// ffmpeg `-hwaccels` output entries.
    pub ffmpeg_hwaccels: Vec<String>,
    /// Failures/diagnostics per tested backend.
    pub notes: Vec<String>,
    /// Backends that failed at runtime (ffmpeg exited early) and are no longer used.
    #[serde(default)]
    pub degraded: Vec<String>,
}

impl HwAccelStatus {
    pub fn software() -> Self {
        HwAccelStatus {
            selected: HwAccel::Software,
            available: vec![],
            ffmpeg_hwaccels: vec![],
            notes: vec![],
            degraded: vec![],
        }
    }
}

/// Parse `ffmpeg -hwaccels` output (first line is a header).
pub fn parse_hwaccels(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with(':'))
        .map(String::from)
        .collect()
}

/// Parse `ffmpeg -encoders` output into the set of encoder names.
pub fn parse_encoders(out: &str) -> HashSet<String> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let flags = it.next()?;
            let name = it.next()?;
            // flags look like "V....D"; skip the "------" separator and header rows
            (name != "="
                && flags.len() == 6
                && flags.chars().all(|c| c.is_ascii_uppercase() || c == '.'))
            .then(|| name.to_string())
        })
        .collect()
}

/// Arguments for the 1-frame test encode of a backend.
pub fn test_encode_args(hw: &HwAccel) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match hw {
        HwAccel::Vaapi { device } => push(&[
            "-init_hw_device",
            &format!("vaapi=va:{device}"),
            "-filter_hw_device",
            "va",
        ]),
        HwAccel::Qsv { device } => push(&[
            "-init_hw_device",
            &format!("vaapi=va:{device}"),
            "-init_hw_device",
            "qsv=qs@va",
            "-filter_hw_device",
            "qs",
        ]),
        _ => {}
    }
    push(&["-f", "lavfi", "-i", "color=c=black:s=256x256:d=0.2:r=10"]);
    match hw {
        HwAccel::Vaapi { .. } => push(&["-vf", "format=nv12,hwupload"]),
        HwAccel::Qsv { .. } => push(&["-vf", "format=nv12,hwupload=extra_hw_frames=16"]),
        _ => push(&["-vf", "format=yuv420p"]),
    }
    push(&[
        "-frames:v",
        "1",
        "-c:v",
        hw.h264_encoder(),
        "-f",
        "null",
        "-",
    ]);
    a
}

async fn run(ffmpeg: &str, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(ffmpeg)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn test_encode(ffmpeg: &str, hw: &HwAccel) -> Result<(), String> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(ffmpeg)
            .args(test_encode_args(hw))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "timed out".to_string())?
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("failed")
            .to_string())
    }
}

/// Probe ffmpeg for usable hardware encoders; falls back to software.
///
/// Preference order: VideoToolbox, NVENC, VAAPI, QSV, software.
pub async fn detect_with(ffmpeg: &str, vaapi_device: &str) -> HwAccelStatus {
    let mut status = HwAccelStatus::software();
    let Some(hw_out) = run(ffmpeg, &["-hide_banner", "-hwaccels"]).await else {
        status.notes.push("ffmpeg not runnable".into());
        return status;
    };
    status.ffmpeg_hwaccels = parse_hwaccels(&hw_out);
    let encoders = run(ffmpeg, &["-hide_banner", "-encoders"])
        .await
        .map(|o| parse_encoders(&o))
        .unwrap_or_default();

    let candidates = [
        HwAccel::VideoToolbox,
        HwAccel::Nvenc,
        HwAccel::Vaapi {
            device: vaapi_device.to_string(),
        },
        HwAccel::Qsv {
            device: vaapi_device.to_string(),
        },
    ];
    for c in candidates {
        if !encoders.contains(c.h264_encoder()) {
            continue;
        }
        if matches!(c, HwAccel::Vaapi { .. } | HwAccel::Qsv { .. })
            && !std::path::Path::new(vaapi_device).exists()
        {
            status
                .notes
                .push(format!("{}: {vaapi_device} missing", c.name()));
            continue;
        }
        match test_encode(ffmpeg, &c).await {
            Ok(()) => status.available.push(c),
            Err(e) => status.notes.push(format!("{}: {e}", c.name())),
        }
    }
    if let Some(first) = status.available.first() {
        status.selected = first.clone();
    }
    status
}

/// Detect using `ffmpeg` from `PATH` and the default VAAPI render node.
pub async fn detect() -> HwAccelStatus {
    detect_with("ffmpeg", DEFAULT_VAAPI_DEVICE).await
}

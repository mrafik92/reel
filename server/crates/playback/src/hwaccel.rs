//! Hardware acceleration detection (ffmpeg subprocess probing).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use utoipa::ToSchema;

pub const DEFAULT_VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// Name under which a failed low-power encoder is recorded in [`HwAccelStatus::degraded`].
pub const VAAPI_LOW_POWER: &str = "vaapi-lp";

/// VAAPI low-power (fixed-function, `VAEntrypointEncSliceLP`) encoders that passed a test encode at
/// startup. Used with `-low_power 1`; Intel's VDEnc roughly doubles HEVC/ladder throughput on the
/// N100 (#102, #113).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LowPower {
    pub h264: bool,
    pub hevc: bool,
}

impl LowPower {
    pub fn any(&self) -> bool {
        self.h264 || self.hevc
    }
    /// Low-power support for an output codec (`"hevc"`, anything else is H.264).
    pub fn for_codec(&self, codec: &str) -> bool {
        if codec == "hevc" {
            self.hevc
        } else {
            self.h264
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HwAccel {
    Vaapi {
        device: String,
        /// Low-power encoders to use (`-low_power 1`); the regular encoders otherwise.
        #[serde(default)]
        low_power: LowPower,
    },
    Qsv {
        device: String,
    },
    Nvenc,
    VideoToolbox,
    Software,
}

impl HwAccel {
    /// VAAPI on `device` with the regular (non-low-power) encoders.
    pub fn vaapi(device: impl Into<String>) -> Self {
        HwAccel::Vaapi {
            device: device.into(),
            low_power: LowPower::default(),
        }
    }

    /// Low-power encoders this backend uses (none for anything but VAAPI).
    pub fn low_power(&self) -> LowPower {
        match self {
            HwAccel::Vaapi { low_power, .. } => *low_power,
            _ => LowPower::default(),
        }
    }

    /// The same backend with the low-power encoders switched off (`None`: nothing to switch off).
    pub fn without_low_power(&self) -> Option<HwAccel> {
        match self {
            HwAccel::Vaapi { device, low_power } if low_power.any() => {
                Some(HwAccel::vaapi(device.clone()))
            }
            _ => None,
        }
    }

    /// ffmpeg video encoder for an output codec (`"hevc"`, anything else is H.264).
    /// Software HEVC encoding is never offered (CPU-exhaustion vector): H.264 instead.
    pub fn video_encoder(&self, codec: &str) -> &'static str {
        match (codec, self) {
            ("hevc", HwAccel::Vaapi { .. }) => "hevc_vaapi",
            ("hevc", HwAccel::Qsv { .. }) => "hevc_qsv",
            ("hevc", HwAccel::Nvenc) => "hevc_nvenc",
            ("hevc", HwAccel::VideoToolbox) => "hevc_videotoolbox",
            (_, hw) => hw.h264_encoder(),
        }
    }

    /// Human-readable encoder for an output codec, e.g. `h264_vaapi (low power)` (admin display).
    pub fn encoder_label(&self, codec: &str) -> String {
        let enc = self.video_encoder(codec);
        if self.low_power().for_codec(codec) {
            format!("{enc} (low power)")
        } else {
            enc.to_string()
        }
    }

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
    encode_probe_args(hw, hw.h264_encoder(), false)
}

/// Arguments for the low-power test encode of `encoder` (`h264_vaapi` / `hevc_vaapi`). It uses the
/// rate control of real transcodes (VBR at a bitrate): low-power bitrate control needs the GPU's HuC
/// firmware, so a constant-QP probe could pass where real sessions would fail.
pub fn low_power_test_args(device: &str, encoder: &str) -> Vec<String> {
    encode_probe_args(&HwAccel::vaapi(device), encoder, true)
}

fn encode_probe_args(hw: &HwAccel, encoder: &str, low_power: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match hw {
        HwAccel::Vaapi { device, .. } => push(&[
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
    push(&["-frames:v", "1", "-c:v", encoder]);
    if low_power {
        push(&[
            "-low_power",
            "1",
            "-rc_mode",
            "VBR",
            "-b:v",
            "2000k",
            "-maxrate",
            "2400k",
        ]);
    }
    push(&["-f", "null", "-"]);
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

async fn test_encode(ffmpeg: &str, args: Vec<String>) -> Result<(), String> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(ffmpeg)
            .args(args)
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
        Err(crate::redact::redact_text(
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("failed"),
        ))
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
        HwAccel::vaapi(vaapi_device),
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
        match test_encode(ffmpeg, test_encode_args(&c)).await {
            Ok(()) => {
                let c = match c {
                    HwAccel::Vaapi { device, .. } => {
                        let low_power =
                            probe_low_power(ffmpeg, &device, &encoders, &mut status.notes).await;
                        HwAccel::Vaapi { device, low_power }
                    }
                    c => c,
                };
                status.available.push(c)
            }
            Err(e) => status.notes.push(format!("{}: {e}", c.name())),
        }
    }
    if let Some(first) = status.available.first() {
        status.selected = first.clone();
    }
    status
}

/// Test-encode the low-power VAAPI encoders (H.264 and HEVC separately). Each result is noted;
/// `REEL_VAAPI_LOW_POWER=0` skips the probe and keeps the regular encoders.
async fn probe_low_power(
    ffmpeg: &str,
    device: &str,
    encoders: &HashSet<String>,
    notes: &mut Vec<String>,
) -> LowPower {
    if std::env::var("REEL_VAAPI_LOW_POWER").is_ok_and(|v| low_power_disabled(&v)) {
        notes.push("vaapi: low-power encoders disabled (REEL_VAAPI_LOW_POWER)".into());
        return LowPower::default();
    }
    let mut lp = LowPower::default();
    for (encoder, flag) in [("h264_vaapi", &mut lp.h264), ("hevc_vaapi", &mut lp.hevc)] {
        if !encoders.contains(encoder) {
            continue;
        }
        match test_encode(ffmpeg, low_power_test_args(device, encoder)).await {
            Ok(()) => {
                *flag = true;
                notes.push(format!("vaapi: {encoder} low power available"));
            }
            Err(e) => notes.push(format!("vaapi: {encoder} low power unavailable: {e}")),
        }
    }
    lp
}

/// `REEL_VAAPI_LOW_POWER` values that switch the low-power encoders off.
pub fn low_power_disabled(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Detect using `ffmpeg` from `PATH` and the default VAAPI render node.
pub async fn detect() -> HwAccelStatus {
    detect_with("ffmpeg", DEFAULT_VAAPI_DEVICE).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(h264: bool, hevc: bool) -> HwAccel {
        HwAccel::Vaapi {
            device: DEFAULT_VAAPI_DEVICE.into(),
            low_power: LowPower { h264, hevc },
        }
    }

    #[test]
    fn low_power_probe_uses_vbr_like_real_transcodes() {
        let a = low_power_test_args(DEFAULT_VAAPI_DEVICE, "hevc_vaapi").join(" ");
        assert!(
            a.contains("-init_hw_device vaapi=va:/dev/dri/renderD128"),
            "{a}"
        );
        assert!(
            a.contains(
                "-c:v hevc_vaapi -low_power 1 -rc_mode VBR -b:v 2000k -maxrate 2400k -f null -"
            ),
            "{a}"
        );
        // the regular probe stays as it was
        let a = test_encode_args(&HwAccel::vaapi(DEFAULT_VAAPI_DEVICE)).join(" ");
        assert!(a.ends_with("-frames:v 1 -c:v h264_vaapi -f null -"), "{a}");
    }

    #[test]
    fn encoder_labels_and_low_power_removal() {
        assert_eq!(
            lp(true, false).encoder_label("h264"),
            "h264_vaapi (low power)"
        );
        assert_eq!(lp(true, false).encoder_label("hevc"), "hevc_vaapi");
        assert_eq!(
            lp(false, true).encoder_label("hevc"),
            "hevc_vaapi (low power)"
        );
        assert_eq!(HwAccel::Software.encoder_label("hevc"), "libx264");
        assert_eq!(
            lp(true, true).without_low_power(),
            Some(HwAccel::vaapi(DEFAULT_VAAPI_DEVICE))
        );
        assert_eq!(lp(false, false).without_low_power(), None);
        assert_eq!(HwAccel::Nvenc.without_low_power(), None);
    }

    #[test]
    fn vaapi_without_low_power_field_still_deserializes() {
        let hw: HwAccel =
            serde_json::from_str(r#"{"type":"vaapi","device":"/dev/dri/renderD128"}"#).unwrap();
        assert_eq!(hw, HwAccel::vaapi("/dev/dri/renderD128"));
        let json = serde_json::to_string(&lp(true, false)).unwrap();
        assert!(
            json.contains(r#""low_power":{"h264":true,"hevc":false}"#),
            "{json}"
        );
    }

    #[test]
    fn low_power_env_values() {
        for v in ["0", "false", "OFF", " no "] {
            assert!(low_power_disabled(v), "{v}");
        }
        for v in ["1", "true", "on", ""] {
            assert!(!low_power_disabled(v), "{v}");
        }
    }
}

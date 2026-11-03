//! Adaptive quality: resolve a requested quality / client bandwidth hint into a playback
//! decision (original streams when they fit, otherwise a fixed H.264 ladder).

use crate::decision::{
    AudioAction, DecisionInput, PlaybackDecision, PlaybackMode, SubtitleAction, VideoAction,
    decide, fit, is_image_subtitle,
};
use crate::model::{Hdr, StreamKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Requested / resolved quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub enum Quality {
    /// Original when it fits the bandwidth cap, else the best ladder rung that does.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// Direct play / remux / audio-only transcode (only a hard user limit can override it).
    #[serde(rename = "original")]
    Original,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

impl Quality {
    pub fn id(self) -> &'static str {
        match self {
            Quality::Auto => "auto",
            Quality::Original => "original",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::P480 => "480p",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QualityRequest {
    pub quality: Quality,
    /// Client bandwidth hint (kbit/s). The effective cap is `min(hint, user limit)`.
    pub max_bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct QualityOption {
    pub id: Quality,
    pub label: String,
    /// Expected total bitrate (kbit/s) of this choice.
    pub bitrate_kbps: u32,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct QualityDecision {
    pub decision: PlaybackDecision,
    /// What was actually chosen: `original` or a ladder rung (never `auto`).
    pub quality: Quality,
    /// Expected bitrate (kbit/s) of the chosen output.
    pub bitrate_kbps: u32,
    pub options: Vec<QualityOption>,
}

struct Rung {
    q: Quality,
    w: u32,
    h: u32,
    video_kbps: u32,
    audio_kbps: u32,
}

/// Highest first.
const LADDER: [Rung; 3] = [
    Rung {
        q: Quality::P1080,
        w: 1920,
        h: 1080,
        video_kbps: 8000,
        audio_kbps: 256,
    },
    Rung {
        q: Quality::P720,
        w: 1280,
        h: 720,
        video_kbps: 4000,
        audio_kbps: 160,
    },
    Rung {
        q: Quality::P480,
        w: 854,
        h: 480,
        video_kbps: 2000,
        audio_kbps: 128,
    },
];

const MIN_VIDEO_KBPS: u32 = 300;

fn mbps(kbps: u32) -> String {
    format!("{:.1} Mbps", kbps as f64 / 1000.0)
}

/// Source video dims/HDR of the stream `decide` would use.
fn source_video(input: &DecisionInput<'_>) -> (u32, u32, Hdr) {
    input
        .streams
        .iter()
        .find(|s| {
            s.kind == StreamKind::Video
                && !matches!(s.codec.as_deref(), Some("mjpeg") | Some("png"))
        })
        .map(|v| {
            (
                v.width.unwrap_or(0).max(0) as u32,
                v.height.unwrap_or(0).max(0) as u32,
                v.hdr,
            )
        })
        .unwrap_or((0, 0, Hdr::None))
}

fn source_kbps(input: &DecisionInput<'_>) -> Option<u32> {
    input
        .file
        .bitrate
        .filter(|b| *b > 0)
        .map(|b| (b / 1000) as u32)
}

/// Never upscale: a rung is offered if the source is at least ~its size in either dimension.
fn rung_available(r: &Rung, w: u32, h: u32) -> bool {
    (w == 0 && h == 0) || w as f64 >= r.w as f64 * 0.9 || h as f64 >= r.h as f64 * 0.9
}

/// A rung that would neither shrink the picture nor the bitrate adds nothing over the original.
fn rung_pointless(r: &Rung, w: u32, h: u32, src_kbps: Option<u32>, orig_kbps: u32) -> bool {
    let no_downscale = w > 0 && h > 0 && fit(w, h, r.w, r.h) == (w & !1, h & !1);
    no_downscale && src_kbps.is_some() && orig_kbps <= r.video_kbps + r.audio_kbps
}

fn rung_total(r: &Rung, src_kbps: Option<u32>) -> u32 {
    let v = src_kbps.map_or(r.video_kbps, |s| r.video_kbps.min(s));
    v + r.audio_kbps
}

/// Expected total bitrate of an already decided (non-rung) output.
fn expected_kbps(d: &PlaybackDecision, src_kbps: Option<u32>) -> u32 {
    match &d.video {
        VideoAction::Encode { bitrate_kbps, .. } => {
            *bitrate_kbps
                + match &d.audio {
                    AudioAction::Encode { bitrate_kbps, .. } => *bitrate_kbps,
                    AudioAction::Copy => 192,
                    AudioAction::None => 0,
                }
        }
        _ => src_kbps.unwrap_or(0),
    }
}

/// Decide with a quality request. `input.max_bitrate_kbps` is the *user limit* (hard cap).
pub fn decide_quality(input: &DecisionInput<'_>, req: &QualityRequest) -> QualityDecision {
    let user_cap = input.max_bitrate_kbps;
    // the baseline: how the file would play with no bandwidth constraint at all
    let base = decide(&DecisionInput {
        file: input.file,
        streams: input.streams,
        audio_index: input.audio_index,
        subtitle_index: input.subtitle_index,
        profile: input.profile,
        max_bitrate_kbps: None,
    });
    let (sw, sh, hdr) = source_video(input);
    let src_kbps = source_kbps(input);
    let orig_kbps = expected_kbps(&base, src_kbps);
    let has_video = !matches!(base.video, VideoAction::None);

    let hint = if req.quality == Quality::Original {
        None
    } else {
        req.max_bitrate_kbps
    };
    let cap = [user_cap, hint, input.profile.max_bitrate_kbps]
        .into_iter()
        .flatten()
        .min();
    let fits = |k: u32| cap.is_none_or(|c| k <= c);

    // option table
    let rung_ok = |r: &Rung| {
        has_video
            && rung_available(r, sw, sh)
            && !(fits(orig_kbps) && rung_pointless(r, sw, sh, src_kbps, orig_kbps))
            && user_cap.is_none_or(|c| rung_total(r, src_kbps) <= c || r.q == Quality::P480)
    };

    // choose
    let step_from = |start: usize| -> Option<usize> {
        let usable: Vec<usize> = (start..LADDER.len())
            .filter(|&i| rung_ok(&LADDER[i]))
            .collect();
        usable
            .iter()
            .copied()
            .find(|&i| fits(rung_total(&LADDER[i], src_kbps)))
            .or_else(|| usable.last().copied())
    };
    let mut reason: Option<String> = None;
    let chosen: Option<usize> = if !has_video || fits(orig_kbps) {
        match req.quality {
            Quality::P1080 | Quality::P720 | Quality::P480 => {
                // an explicit rung that is not offered (would upscale / change nothing) means
                // "no preference": keep the original
                let i = LADDER.iter().position(|r| r.q == req.quality).unwrap_or(0);
                let ok = rung_ok(&LADDER[i]) && fits(rung_total(&LADDER[i], src_kbps));
                if ok {
                    reason = Some(format!("quality {} requested", req.quality.id()));
                }
                ok.then_some(i)
            }
            _ => None,
        }
    } else {
        let start = match req.quality {
            Quality::P1080 | Quality::P720 | Quality::P480 => {
                LADDER.iter().position(|r| r.q == req.quality).unwrap_or(0)
            }
            _ => 0,
        };
        // explicit but unavailable rung over the cap: continue below it (cap still applies)
        let c = step_from(start);
        if c.is_some() {
            reason = Some(format!(
                "source needs ~{} but the cap is {} ({})",
                mbps(orig_kbps),
                mbps(cap.unwrap_or(0)),
                if hint.is_some_and(|h| Some(h) == cap) {
                    "client bandwidth"
                } else {
                    "user limit"
                }
            ));
        }
        c
    };

    let (decision, quality, bitrate_kbps) = match chosen {
        Some(i) => {
            let r = &LADDER[i];
            let d = apply_rung(
                &base,
                input,
                r,
                cap,
                src_kbps,
                hdr,
                reason.unwrap_or_default(),
            );
            let total = expected_kbps(&d, src_kbps);
            (d, r.q, total)
        }
        None => {
            let q = if matches!(base.video, VideoAction::Encode { .. }) {
                quality_for_dims(&base, sw, sh)
            } else {
                Quality::Original
            };
            (base.clone(), q, orig_kbps)
        }
    };

    let mut options = vec![QualityOption {
        id: Quality::Auto,
        label: "Auto".into(),
        bitrate_kbps,
        available: true,
    }];
    options.push(QualityOption {
        id: Quality::Original,
        label: if orig_kbps > 0 {
            format!("Original ({})", mbps(orig_kbps))
        } else {
            "Original".into()
        },
        bitrate_kbps: orig_kbps,
        available: user_cap.is_none_or(|c| orig_kbps <= c),
    });
    for r in &LADDER {
        let total = rung_total(r, src_kbps);
        options.push(QualityOption {
            id: r.q,
            label: format!("{} ({})", r.q.id(), mbps(total)),
            bitrate_kbps: total,
            available: rung_ok(r),
        });
    }
    QualityDecision {
        decision,
        quality,
        bitrate_kbps,
        options,
    }
}

/// Resolved id of a compatibility transcode that was not forced by quality.
fn quality_for_dims(d: &PlaybackDecision, sw: u32, sh: u32) -> Quality {
    if let VideoAction::Encode { width, height, .. } = &d.video {
        if *width as u64 * *height as u64 + 1 >= sw as u64 * sh as u64 && sw > 0 {
            return Quality::Original;
        }
        return match *width {
            1500.. => Quality::P1080,
            1000.. => Quality::P720,
            _ => Quality::P480,
        };
    }
    Quality::Original
}

fn apply_rung(
    base: &PlaybackDecision,
    input: &DecisionInput<'_>,
    r: &Rung,
    cap: Option<u32>,
    src_kbps: Option<u32>,
    hdr: Hdr,
    why: String,
) -> PlaybackDecision {
    let profile = input.profile;
    let (sw, sh, _) = source_video(input);
    let (bw, bh) = (r.w.min(profile.max_width), r.h.min(profile.max_height));
    let (w, h) = if sw > 0 && sh > 0 {
        fit(sw, sh, bw, bh)
    } else {
        (bw & !1, bh & !1)
    };
    let codec = if profile.video_support("h264").is_some() {
        "h264".to_string()
    } else {
        profile
            .video
            .first()
            .map(|v| v.codec.clone())
            .unwrap_or_else(|| "h264".into())
    };
    let tonemap = hdr != Hdr::None
        && !profile
            .video_support(&codec)
            .is_some_and(|s| s.hdr.contains(&hdr));
    let mut video = src_kbps.map_or(r.video_kbps, |s| r.video_kbps.min(s));
    if let Some(c) = cap {
        video = video.min(((c as f64 * 0.95) as u32).saturating_sub(r.audio_kbps));
    }
    let video = video.max(MIN_VIDEO_KBPS);

    let audio = match &base.audio {
        AudioAction::None => AudioAction::None,
        // 720p/480p: stereo AAC; 1080p keeps the original layout (copy/transcode as decided)
        _ if r.q != Quality::P1080 => AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: r.audio_kbps,
        },
        a => a.clone(),
    };
    let subtitle = match &base.subtitle {
        SubtitleAction::Native => {
            let image = base
                .subtitle_index
                .and_then(|i| {
                    input
                        .streams
                        .iter()
                        .find(|s| s.kind == StreamKind::Subtitle && s.idx == i)
                })
                .is_some_and(|s| is_image_subtitle(s.codec.as_deref().unwrap_or("")));
            if image {
                SubtitleAction::BurnIn
            } else {
                SubtitleAction::Sidecar {
                    format: "vtt".into(),
                }
            }
        }
        other => other.clone(),
    };
    let mut reasons: Vec<String> = base
        .reasons
        .iter()
        .filter(|x| !x.starts_with("client supports container"))
        .cloned()
        .collect();
    reasons.push(format!(
        "adaptive quality {}: {}",
        r.q.id(),
        if why.is_empty() {
            "transcode".into()
        } else {
            why
        }
    ));
    PlaybackDecision {
        mode: PlaybackMode::Transcode,
        video: VideoAction::Encode {
            codec,
            width: w,
            height: h,
            bitrate_kbps: video,
            tonemap,
        },
        audio,
        subtitle,
        reasons,
        video_index: base.video_index,
        audio_index: base.audio_index,
        subtitle_index: base.subtitle_index,
    }
}

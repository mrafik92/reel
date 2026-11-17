//! Variant sets for multi-variant HLS (#111): which renditions a session lists, how they are
//! encoded, and which one the player starts on. Pure functions; the session manager runs them.
//!
//! - A **copyable original** (remux / audio-only transcode) is listed as `original`, plus every
//!   ladder rung that doesn't upscale and costs ≤ 0.7× the original's bitrate (at most three),
//!   encoded in the original's codec family (hls.js ABR never switches across codec fourCCs) with
//!   the original's audio, when a transcode slot is free.
//! - A **transcode-only** file lists its top rung (what decide picks with no bandwidth hint) and,
//!   when two transcode slots are free, the lower H.264 rungs.
//! - Variants above the user's bitrate limit are left out (480p always stays).

use crate::codecs;
use crate::decision::{
    AudioAction, PlaybackDecision, PlaybackMode, SubtitleAction, VideoAction, fit,
    is_image_subtitle, normalize_codec,
};
use crate::hwaccel::HwAccel;
use crate::model::{FileInfo, StreamInfo, StreamKind};
use crate::quality::Quality;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the master playlist says about a variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VariantMeta {
    /// Shown to people ("Original", "720p").
    pub label: String,
    pub width: u32,
    pub height: u32,
    /// `AVERAGE-BANDWIDTH` (kbit/s, video + audio).
    pub average_kbps: u32,
    /// `BANDWIDTH`: the peak (kbit/s, video + audio).
    pub peak_kbps: u32,
    /// RFC 6381 `CODECS` value; `None` when a track can't be described (the attribute is left out).
    pub codecs: Option<String>,
}

/// One planned variant: its URL id, how it is produced and how it is described.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedVariant {
    /// `original`, `1080p`, `720p` or `480p`: the URL path segment and the quality id.
    pub id: Quality,
    pub decision: PlaybackDecision,
    pub meta: VariantMeta,
}

impl PlannedVariant {
    /// The picture is re-encoded.
    pub fn transcode(&self) -> bool {
        matches!(self.decision.video, VideoAction::Encode { .. })
    }
}

/// Inputs of [`plan_variants`].
pub struct VariantInput<'a> {
    pub file: &'a FileInfo,
    pub streams: &'a [StreamInfo],
    /// The decision with no bandwidth hint (user limit and device applied): the original when
    /// it can be copied, else the top rung. Must need ffmpeg (not direct play).
    pub top: &'a PlaybackDecision,
    /// The user's bitrate limit (kbit/s).
    pub user_cap_kbps: Option<u32>,
    /// Backend the rungs will be encoded with (decides whether HEVC rungs are possible).
    pub hw: &'a HwAccel,
    /// Transcode slots free right now (global and per user, live TV included).
    pub free_transcode_slots: usize,
}

struct Rung {
    q: Quality,
    w: u32,
    h: u32,
    video_kbps: u32,
}

const H264_LADDER: [Rung; 3] = [
    Rung {
        q: Quality::P1080,
        w: 1920,
        h: 1080,
        video_kbps: 8000,
    },
    Rung {
        q: Quality::P720,
        w: 1280,
        h: 720,
        video_kbps: 4000,
    },
    Rung {
        q: Quality::P480,
        w: 854,
        h: 480,
        video_kbps: 2000,
    },
];

/// About 0.7× the H.264 bitrates (#113 measurements; Q3 of the #111 design).
const HEVC_LADDER: [Rung; 3] = [
    Rung {
        q: Quality::P1080,
        w: 1920,
        h: 1080,
        video_kbps: 5000,
    },
    Rung {
        q: Quality::P720,
        w: 1280,
        h: 720,
        video_kbps: 2800,
    },
    Rung {
        q: Quality::P480,
        w: 854,
        h: 480,
        video_kbps: 1400,
    },
];

/// A rung must cost at most this share of the variant above it to be worth listing.
const RUNG_SHARE: f64 = 0.7;
/// `BANDWIDTH` of a copied original when only its average bitrate is known: a VBR film peaks
/// well above its average (The Remarried Empress: 5.9 Mbit/s average, 11–13 at its peaks).
const ORIGINAL_PEAK_FACTOR: f64 = 1.7;
/// Assumed bitrate of a copied audio track whose rate isn't probed.
const COPIED_AUDIO_KBPS: u32 = 192;

fn selected(streams: &[StreamInfo], idx: Option<i64>, kind: StreamKind) -> Option<&StreamInfo> {
    streams
        .iter()
        .find(|s| Some(s.idx) == idx && s.kind == kind)
}

fn audio_kbps(d: &PlaybackDecision) -> u32 {
    match &d.audio {
        AudioAction::Encode { bitrate_kbps, .. } => *bitrate_kbps,
        AudioAction::Copy => COPIED_AUDIO_KBPS,
        AudioAction::None => 0,
    }
}

/// `CODECS` part of the decision's audio track (`None` inside: present but not describable).
fn audio_codecs(d: &PlaybackDecision, streams: &[StreamInfo]) -> Option<Option<String>> {
    match &d.audio {
        AudioAction::None => None,
        AudioAction::Encode { codec, .. } => Some(codecs::audio(codec, None)),
        AudioAction::Copy => {
            let a = selected(streams, d.audio_index, StreamKind::Audio);
            Some(a.and_then(|a| codecs::audio(a.codec.as_deref()?, a.profile.as_deref())))
        }
    }
}

/// The rung ladder for a codec family, if Reel can encode it on this backend.
fn ladder_for(family: &str, hw: &HwAccel) -> Option<(&'static str, &'static [Rung; 3])> {
    match family {
        "h264" => Some(("h264", &H264_LADDER)),
        // only with a hardware HEVC encoder: software falls back to libx264 (another family)
        "hevc" if hw.video_encoder("hevc").starts_with("hevc") => Some(("hevc", &HEVC_LADDER)),
        _ => None,
    }
}

/// Never upscale: offered when the source is at least about the rung's size in one dimension.
fn fits_under(r: &Rung, w: u32, h: u32) -> bool {
    w > 0 && h > 0 && (w as f64 >= r.w as f64 * 0.9 || h as f64 >= r.h as f64 * 0.9)
}

fn label(q: Quality) -> String {
    match q {
        Quality::Original | Quality::Auto => "Original".into(),
        other => other.id().into(),
    }
}

/// The quality class of an encoded picture (for a transcode-only top rung's id).
fn class_of(h: u32) -> Quality {
    match h {
        900.. => Quality::P1080,
        620.. => Quality::P720,
        _ => Quality::P480,
    }
}

/// A rung decision: the base with its video replaced (audio, subtitle, tone-map and stream
/// indexes stay the same, so every variant has identical audio).
fn rung_decision(
    base: &PlaybackDecision,
    codec: &str,
    (w, h): (u32, u32),
    video_kbps: u32,
    tonemap: bool,
    q: Quality,
) -> PlaybackDecision {
    let mut d = base.clone();
    d.mode = PlaybackMode::Transcode;
    d.video = VideoAction::Encode {
        codec: codec.into(),
        width: w,
        height: h,
        bitrate_kbps: video_kbps,
        tonemap,
    };
    d.reasons.push(format!("adaptive rung {}", q.id()));
    d
}

fn encoded_meta(
    q: Quality,
    d: &PlaybackDecision,
    streams: &[StreamInfo],
    ten_bit: bool,
) -> VariantMeta {
    let (codec, w, h, v) = match &d.video {
        VideoAction::Encode {
            codec,
            width,
            height,
            bitrate_kbps,
            ..
        } => (codec.as_str(), *width, *height, *bitrate_kbps),
        _ => ("h264", 0, 0, 0),
    };
    let a = audio_kbps(d);
    VariantMeta {
        label: label(q),
        width: w,
        height: h,
        average_kbps: v + a,
        // the encoder's maxrate (b + b/5)
        peak_kbps: v + v / 5 + a,
        codecs: codecs::attribute(
            Some(codecs::encoded_video(codec, w, h, ten_bit)),
            audio_codecs(d, streams),
        ),
    }
}

fn under_cap(m: &VariantMeta, q: Quality, cap: Option<u32>) -> bool {
    q == Quality::P480 || cap.is_none_or(|c| m.average_kbps <= c)
}

/// The variants to list for a session, ascending by peak bandwidth (master order). Empty when
/// `top` doesn't need ffmpeg; one entry when no rung qualifies or no slot is free.
pub fn plan_variants(i: &VariantInput<'_>) -> Vec<PlannedVariant> {
    let top = i.top;
    if !top.mode.needs_ffmpeg() || top.video_index.is_none() {
        return Vec::new();
    }
    let src = selected(i.streams, top.video_index, StreamKind::Video);
    let (sw, sh) = src
        .map(|v| {
            (
                v.width.unwrap_or(0).max(0) as u32,
                v.height.unwrap_or(0).max(0) as u32,
            )
        })
        .unwrap_or((0, 0));
    let mut out: Vec<PlannedVariant> = Vec::new();
    match &top.video {
        VideoAction::Copy => {
            let Some(v) = src else {
                return out;
            };
            let src_kbps = i.file.bitrate.filter(|b| *b > 0).map(|b| (b / 1000) as u32);
            // the file's bitrate includes the audio it carries; a re-encoded audio track replaces it
            let avg = src_kbps.unwrap_or(0);
            let ten_bit = v.bit_depth.is_some_and(|d| d > 8);
            let original = PlannedVariant {
                id: Quality::Original,
                decision: top.clone(),
                meta: VariantMeta {
                    label: label(Quality::Original),
                    width: sw,
                    height: sh,
                    average_kbps: avg,
                    peak_kbps: (avg as f64 * ORIGINAL_PEAK_FACTOR).round() as u32,
                    codecs: codecs::attribute(
                        Some(codecs::copied_video(
                            v.codec.as_deref().unwrap_or(""),
                            v.profile.as_deref(),
                            v.level,
                            v.bit_depth,
                            sw,
                            sh,
                        )),
                        audio_codecs(top, i.streams),
                    ),
                },
            };
            let family = normalize_codec(v.codec.as_deref().unwrap_or(""));
            if let (Some((codec, ladder)), true, true) = (
                ladder_for(&family, i.hw),
                src_kbps.is_some(),
                i.free_transcode_slots >= 1,
            ) {
                let a = audio_kbps(top);
                for r in ladder.iter() {
                    let total = r.video_kbps + a;
                    if !fits_under(r, sw, sh) || total as f64 > avg as f64 * RUNG_SHARE {
                        continue;
                    }
                    let d =
                        rung_decision(top, codec, fit(sw, sh, r.w, r.h), r.video_kbps, false, r.q);
                    let meta = encoded_meta(r.q, &d, i.streams, ten_bit && codec == "hevc");
                    if under_cap(&meta, r.q, i.user_cap_kbps) {
                        out.push(PlannedVariant {
                            id: r.q,
                            decision: d,
                            meta,
                        });
                    }
                }
            }
            if under_cap(&original.meta, Quality::Original, i.user_cap_kbps) {
                out.push(original);
            }
        }
        VideoAction::Encode {
            codec,
            width,
            height,
            bitrate_kbps,
            tonemap,
        } => {
            let q = class_of(*height);
            let top_meta = encoded_meta(q, top, i.streams, false);
            let top_total = top_meta.average_kbps;
            if i.free_transcode_slots >= 2 {
                let a = audio_kbps(top);
                for r in H264_LADDER.iter() {
                    let total = r.video_kbps + a;
                    if r.h as f64 >= *height as f64 * 0.9
                        || total as f64 > top_total as f64 * RUNG_SHARE
                        || r.video_kbps >= *bitrate_kbps
                    {
                        continue;
                    }
                    let dims = fit(*width, *height, r.w, r.h);
                    let d = rung_decision(top, codec, dims, r.video_kbps, *tonemap, r.q);
                    let meta = encoded_meta(r.q, &d, i.streams, false);
                    if r.q != q && under_cap(&meta, r.q, i.user_cap_kbps) {
                        out.push(PlannedVariant {
                            id: r.q,
                            decision: d,
                            meta,
                        });
                    }
                }
            }
            out.push(PlannedVariant {
                id: q,
                decision: top.clone(),
                meta: top_meta,
            });
        }
        VideoAction::None => return out,
    }
    out.sort_by_key(|v| v.meta.peak_kbps);
    out
}

/// A direct-playable file as a remux (the `original` of a multi-variant session, Q1 of the
/// #111 design): same streams, repackaged. `None` when the subtitles would need burning in.
pub fn remux_of_direct(d: &PlaybackDecision, streams: &[StreamInfo]) -> Option<PlaybackDecision> {
    if d.mode != PlaybackMode::DirectPlay || d.video != VideoAction::Copy {
        return None;
    }
    let mut r = d.clone();
    r.mode = PlaybackMode::Remux;
    if r.subtitle == SubtitleAction::Native {
        let sub = selected(streams, d.subtitle_index, StreamKind::Subtitle)?;
        if is_image_subtitle(sub.codec.as_deref().unwrap_or("")) {
            return None;
        }
        r.subtitle = SubtitleAction::Sidecar {
            format: "vtt".into(),
        };
    }
    r.reasons
        .push("remuxed so the player can switch to a lower rung (adaptive streaming)".into());
    Some(r)
}

/// Direct play beats a remux when the link is comfortably faster than the file (Q1).
pub fn direct_play_fits(file: &FileInfo, estimate_kbps: Option<u32>) -> bool {
    match (estimate_kbps, file.bitrate.filter(|b| *b > 0)) {
        (Some(est), Some(b)) => est as f64 >= 1.5 * (b as f64 / 1000.0),
        _ => false,
    }
}

/// Index (into `vs`, ascending) of the variant the player starts on and the server prestarts.
///
/// - Auto: the highest variant whose peak is ≤ 0.8 × the estimate; the lowest without one
///   ("start low, then climb").
/// - Original: `original` when listed.
/// - A rung: the highest variant no taller than it (the player caps there).
pub fn start_index(vs: &[PlannedVariant], quality: Quality, estimate_kbps: Option<u32>) -> usize {
    if vs.is_empty() {
        return 0;
    }
    let by_estimate = || match estimate_kbps {
        Some(e) => vs
            .iter()
            .rposition(|v| v.meta.peak_kbps as f64 <= e as f64 * 0.8)
            .unwrap_or(0),
        None => 0,
    };
    match quality {
        Quality::Original => vs
            .iter()
            .position(|v| v.id == Quality::Original)
            .unwrap_or_else(by_estimate),
        Quality::P1080 | Quality::P720 | Quality::P480 => {
            let max_h = match quality {
                Quality::P1080 => 1080,
                Quality::P720 => 720,
                _ => 480,
            };
            vs.iter()
                .rposition(|v| v.meta.height <= max_h + max_h / 10)
                .unwrap_or(0)
        }
        Quality::Auto => by_estimate(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hwaccel::LowPower;

    fn stream(idx: i64, kind: StreamKind, codec: &str) -> StreamInfo {
        StreamInfo::new(idx, kind, codec)
    }

    fn file(kbps: i64) -> FileInfo {
        FileInfo {
            id: 1,
            path: "/m/a.mkv".into(),
            container: Some("matroska,webm".into()),
            duration_s: Some(7200.0),
            bitrate: Some(kbps * 1000),
        }
    }

    fn video(codec: &str, w: i64, h: i64, depth: i64) -> StreamInfo {
        let mut v = stream(0, StreamKind::Video, codec);
        v.width = Some(w);
        v.height = Some(h);
        v.bit_depth = Some(depth);
        v.profile = Some(if depth > 8 { "Main 10" } else { "High" }.into());
        v
    }

    fn aac() -> StreamInfo {
        let mut a = stream(1, StreamKind::Audio, "aac");
        a.profile = Some("LC".into());
        a
    }

    fn decision(mode: PlaybackMode, video: VideoAction, audio: AudioAction) -> PlaybackDecision {
        PlaybackDecision {
            mode,
            video,
            audio,
            subtitle: SubtitleAction::None,
            reasons: vec![],
            video_index: Some(0),
            audio_index: Some(1),
            subtitle_index: None,
        }
    }

    fn vaapi() -> HwAccel {
        HwAccel::Vaapi {
            device: "/dev/dri/renderD128".into(),
            low_power: LowPower {
                h264: true,
                hevc: true,
            },
        }
    }

    fn ids(vs: &[PlannedVariant]) -> Vec<&'static str> {
        vs.iter().map(|v| v.id.id()).collect()
    }

    fn plan(
        f: &FileInfo,
        streams: &[StreamInfo],
        top: &PlaybackDecision,
        cap: Option<u32>,
        hw: &HwAccel,
        free: usize,
    ) -> Vec<PlannedVariant> {
        plan_variants(&VariantInput {
            file: f,
            streams,
            top,
            user_cap_kbps: cap,
            hw,
            free_transcode_slots: free,
        })
    }

    #[test]
    fn hevc_remux_gets_hevc_rungs_with_its_audio() {
        // Meet Joe Black: HEVC 10-bit 1080p, 6.5 Mbit/s, AAC
        let f = file(6500);
        let s = vec![video("hevc", 1920, 1038, 10), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        let vs = plan(&f, &s, &top, None, &vaapi(), 3);
        assert_eq!(ids(&vs), vec!["480p", "720p", "original"]);
        let o = &vs[2].meta;
        assert_eq!((o.width, o.height, o.average_kbps), (1920, 1038, 6500));
        assert_eq!(o.peak_kbps, 11050);
        assert_eq!(o.codecs.as_deref(), Some("hvc1.2.4.L120.B0,mp4a.40.2"));
        let r720 = &vs[1];
        assert!(r720.transcode());
        assert_eq!(
            r720.decision.video,
            VideoAction::Encode {
                codec: "hevc".into(),
                width: 1280,
                height: 692,
                bitrate_kbps: 2800,
                tonemap: false,
            }
        );
        assert_eq!(
            r720.decision.audio,
            AudioAction::Copy,
            "same audio as the original"
        );
        assert_eq!(r720.decision.mode, PlaybackMode::Transcode);
        assert_eq!(
            r720.meta.codecs.as_deref(),
            Some("hvc1.2.4.L93.B0,mp4a.40.2"),
            "Main10 kept: same fourCC and profile as the original"
        );
        assert_eq!(r720.meta.average_kbps, 2800 + 192);
        assert_eq!(r720.meta.peak_kbps, 2800 + 560 + 192);
        // every variant shares hls.js's codec set
        for v in &vs {
            let c = v.meta.codecs.as_deref().unwrap();
            assert!(c.starts_with("hvc1.") && c.ends_with(",mp4a.40.2"), "{c}");
        }
    }

    #[test]
    fn hevc_without_a_hardware_encoder_lists_only_the_original() {
        let f = file(6500);
        let s = vec![video("hevc", 1920, 1038, 10), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        assert_eq!(
            ids(&plan(&f, &s, &top, None, &HwAccel::Software, 3)),
            vec!["original"]
        );
    }

    #[test]
    fn high_bitrate_h264_gets_three_h264_rungs_and_shared_aac() {
        // Spider-Man: H.264 1080p 24.7 Mbit/s, EAC3 -> audio transcode to AAC
        let f = file(24700);
        let s = vec![
            video("h264", 1920, 1040, 8),
            stream(1, StreamKind::Audio, "eac3"),
        ];
        let aac_enc = AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 160,
        };
        let top = decision(
            PlaybackMode::AudioTranscode,
            VideoAction::Copy,
            aac_enc.clone(),
        );
        let vs = plan(&f, &s, &top, None, &vaapi(), 3);
        assert_eq!(ids(&vs), vec!["480p", "720p", "1080p", "original"]);
        for v in &vs {
            assert_eq!(v.decision.audio, aac_enc);
            let c = v.meta.codecs.as_deref().unwrap();
            assert!(c.starts_with("avc1.") && c.ends_with(",mp4a.40.2"), "{c}");
        }
        assert_eq!(vs[2].meta.height, 1040);
    }

    #[test]
    fn rungs_need_a_free_slot_and_a_real_saving() {
        let s = vec![video("h264", 1920, 1080, 8), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        // no free slot: original only
        assert_eq!(
            ids(&plan(&file(24700), &s, &top, None, &vaapi(), 0)),
            vec!["original"]
        );
        // 2.7 Mbit/s: even 480p (2.19 M) isn't ≤ 0.7× of it
        assert_eq!(
            ids(&plan(&file(2700), &s, &top, None, &vaapi(), 3)),
            vec!["original"]
        );
        // 4 Mbit/s: 480p only
        assert_eq!(
            ids(&plan(&file(4000), &s, &top, None, &vaapi(), 3)),
            vec!["480p", "original"]
        );
        // unknown bitrate: nothing to compare with
        let mut f = file(1);
        f.bitrate = None;
        assert_eq!(
            ids(&plan(&f, &s, &top, None, &vaapi(), 3)),
            vec!["original"]
        );
    }

    #[test]
    fn rungs_never_upscale() {
        let f = file(9000);
        let s = vec![video("h264", 720, 480, 8), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        assert_eq!(
            ids(&plan(&f, &s, &top, None, &vaapi(), 3)),
            vec!["480p", "original"]
        );
    }

    #[test]
    fn user_cap_drops_variants_above_it_but_keeps_480p() {
        let f = file(24700);
        let s = vec![video("h264", 1920, 1080, 8), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        assert_eq!(
            ids(&plan(&f, &s, &top, Some(6000), &vaapi(), 3)),
            vec!["480p", "720p"]
        );
        assert_eq!(
            ids(&plan(&f, &s, &top, Some(500), &vaapi(), 3)),
            vec!["480p"]
        );
    }

    #[test]
    fn av1_original_has_no_rung() {
        let f = file(9000);
        let mut v = video("av1", 1920, 1080, 10);
        v.level = Some(8);
        let s = vec![v, aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        let vs = plan(&f, &s, &top, None, &vaapi(), 3);
        assert_eq!(ids(&vs), vec!["original"]);
        assert_eq!(
            vs[0].meta.codecs.as_deref(),
            Some("av01.0.08M.10,mp4a.40.2")
        );
    }

    #[test]
    fn transcode_only_lists_lower_rungs_only_with_two_free_slots() {
        // 4K DV tone-mapped to 1080p 8 M
        let f = file(25000);
        let s = vec![video("hevc", 3840, 2160, 10), aac()];
        let aac_enc = AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 256,
        };
        let top = decision(
            PlaybackMode::Transcode,
            VideoAction::Encode {
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                bitrate_kbps: 8000,
                tonemap: true,
            },
            aac_enc,
        );
        let vs = plan(&f, &s, &top, None, &vaapi(), 2);
        assert_eq!(ids(&vs), vec!["480p", "720p", "1080p"]);
        for v in &vs {
            let VideoAction::Encode { tonemap, codec, .. } = &v.decision.video else {
                panic!("encode")
            };
            assert!(
                *tonemap && codec == "h264",
                "every rung tone-maps like the top"
            );
            assert!(v.meta.codecs.as_deref().unwrap().starts_with("avc1.64"));
        }
        assert_eq!(vs[2].decision, top);
        assert_eq!(vs[2].meta.peak_kbps, 8000 + 1600 + 256);
        // GPU budget full: one rung, as today
        assert_eq!(ids(&plan(&f, &s, &top, None, &vaapi(), 1)), vec!["1080p"]);
        assert_eq!(ids(&plan(&f, &s, &top, None, &vaapi(), 0)), vec!["1080p"]);
    }

    #[test]
    fn transcode_only_720p_top_gets_only_480p_below() {
        let f = file(25000);
        let s = vec![video("vc1", 1920, 1080, 8), aac()];
        let top = decision(
            PlaybackMode::Transcode,
            VideoAction::Encode {
                codec: "h264".into(),
                width: 1280,
                height: 720,
                bitrate_kbps: 4000,
                tonemap: false,
            },
            AudioAction::Copy,
        );
        assert_eq!(
            ids(&plan(&f, &s, &top, None, &vaapi(), 3)),
            vec!["480p", "720p"]
        );
    }

    #[test]
    fn direct_play_never_gets_variants() {
        let s = vec![video("h264", 1920, 1080, 8), aac()];
        let top = decision(
            PlaybackMode::DirectPlay,
            VideoAction::Copy,
            AudioAction::Copy,
        );
        assert!(plan(&file(9000), &s, &top, None, &vaapi(), 3).is_empty());
    }

    #[test]
    fn direct_play_becomes_a_remux_with_sidecar_subtitles() {
        let mut s = vec![video("h264", 1920, 1080, 8), aac()];
        s.push(stream(2, StreamKind::Subtitle, "subrip"));
        let mut d = decision(
            PlaybackMode::DirectPlay,
            VideoAction::Copy,
            AudioAction::Copy,
        );
        d.subtitle = SubtitleAction::Native;
        d.subtitle_index = Some(2);
        let r = remux_of_direct(&d, &s).unwrap();
        assert_eq!(r.mode, PlaybackMode::Remux);
        assert_eq!(r.video, VideoAction::Copy);
        assert_eq!(r.audio, AudioAction::Copy);
        assert_eq!(
            r.subtitle,
            SubtitleAction::Sidecar {
                format: "vtt".into()
            }
        );
        // picture subtitles would need a burn-in: stay direct play
        s[2] = stream(2, StreamKind::Subtitle, "hdmv_pgs_subtitle");
        assert!(remux_of_direct(&d, &s).is_none());
        // already a remux
        assert!(remux_of_direct(&r, &s).is_none());
    }

    #[test]
    fn direct_play_only_with_a_comfortable_estimate() {
        let f = file(2700);
        assert!(direct_play_fits(&f, Some(4100)));
        assert!(!direct_play_fits(&f, Some(4000)));
        assert!(
            !direct_play_fits(&f, None),
            "first play without an estimate: remux"
        );
    }

    #[test]
    fn start_variant() {
        let f = file(6500);
        let s = vec![video("hevc", 1920, 1038, 10), aac()];
        let top = decision(PlaybackMode::Remux, VideoAction::Copy, AudioAction::Copy);
        let vs = plan(&f, &s, &top, None, &vaapi(), 3);
        // peaks: 480p 1872, 720p 3552, original 11050
        assert_eq!(start_index(&vs, Quality::Auto, None), 0, "start low");
        assert_eq!(start_index(&vs, Quality::Auto, Some(2000)), 0);
        assert_eq!(start_index(&vs, Quality::Auto, Some(4500)), 1);
        assert_eq!(start_index(&vs, Quality::Auto, Some(14000)), 2);
        assert_eq!(start_index(&vs, Quality::Original, None), 2);
        assert_eq!(start_index(&vs, Quality::P720, Some(100_000)), 1);
        assert_eq!(start_index(&vs, Quality::P480, None), 0);
        assert_eq!(
            start_index(&vs, Quality::P1080, None),
            2,
            "1038 lines ≤ 1080"
        );
    }
}

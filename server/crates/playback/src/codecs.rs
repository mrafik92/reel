//! RFC 6381 codec strings for the `CODECS` attribute of an HLS master playlist (#111).
//!
//! hls.js drops a level whose `CODECS` `MediaSource.isTypeSupported` rejects, and its ABR only
//! switches between levels whose codec fourCCs (`avc1`, `hvc1`, `mp4a`, …) match, so every
//! variant of a session must describe its tracks exactly. Copied tracks are described from the
//! probe (`streams.profile`/`level`/`bit_depth`); encoded ones from the encoder settings.

use crate::decision::normalize_codec;

/// H.264 `profile_idc` and constraint byte from an ffprobe profile name.
fn avc_profile(profile: Option<&str>) -> (u8, u8) {
    match profile.map(|p| p.to_ascii_lowercase()).as_deref() {
        Some("constrained baseline") => (0x42, 0xE0),
        Some("baseline") => (0x42, 0x00),
        Some("main") => (0x4D, 0x00),
        Some("extended") => (0x58, 0x00),
        Some("high 10") | Some("high 10 intra") => (0x6E, 0x00),
        Some("high 4:2:2") | Some("high 4:2:2 intra") => (0x7A, 0x00),
        Some(p) if p.starts_with("high 4:4:4") => (0xF4, 0x00),
        _ => (0x64, 0x00), // High
    }
}

/// HEVC level × 30 (`general_level_idc`) for a picture size when the probe has none.
fn hevc_level_for(width: u32, height: u32) -> u32 {
    match width as u64 * height as u64 {
        a if a <= 983_040 => 93,    // 3.1: 720p
        a if a <= 2_228_224 => 120, // 4: 1080p
        _ => 150,                   // 5: 2160p
    }
}

/// H.264 `level_idc` for a picture size (30 fps class) when the probe has none.
fn avc_level_for(width: u32, height: u32) -> u32 {
    match width as u64 * height as u64 {
        a if a <= 921_600 => 31,   // 720p
        a if a <= 2_088_960 => 40, // 1080p
        _ => 51,
    }
}

fn hevc(ten_bit: bool, level: u32) -> String {
    if ten_bit {
        format!("hvc1.2.4.L{level}.B0")
    } else {
        format!("hvc1.1.6.L{level}.B0")
    }
}

/// Codec string of a copied video track, from the probe. `None` when Reel can't describe it
/// (the master then leaves `CODECS` out).
pub fn copied_video(
    codec: &str,
    profile: Option<&str>,
    level: Option<i64>,
    bit_depth: Option<i64>,
    width: u32,
    height: u32,
) -> Option<String> {
    let level = level.filter(|l| *l > 0).map(|l| l as u32);
    match normalize_codec(codec).as_str() {
        "h264" => {
            let (p, c) = avc_profile(profile);
            let l = level.unwrap_or_else(|| avc_level_for(width, height));
            Some(format!("avc1.{p:02X}{c:02X}{l:02X}"))
        }
        "hevc" => {
            let ten = bit_depth.is_some_and(|d| d > 8)
                || profile.is_some_and(|p| p.to_ascii_lowercase().contains("10"));
            Some(hevc(
                ten,
                level.unwrap_or_else(|| hevc_level_for(width, height)),
            ))
        }
        "av1" => {
            let l = level?; // seq_level_idx
            let depth = bit_depth.unwrap_or(8).max(8);
            Some(format!("av01.0.{l:02}M.{depth:02}"))
        }
        "vp9" => {
            let l = level?;
            let depth = bit_depth.unwrap_or(8).max(8);
            Some(format!("vp09.00.{l:02}.{depth:02}"))
        }
        _ => None,
    }
}

/// Codec string of a video track Reel encodes (`h264` High, or `hevc` Main/Main10).
pub fn encoded_video(codec: &str, width: u32, height: u32, ten_bit: bool) -> Option<String> {
    match normalize_codec(codec).as_str() {
        "h264" => Some(format!("avc1.6400{:02X}", avc_level_for(width, height))),
        "hevc" => Some(hevc(ten_bit, hevc_level_for(width, height))),
        _ => None,
    }
}

/// Codec string of an audio track (copied or encoded), from its codec and ffprobe profile.
pub fn audio(codec: &str, profile: Option<&str>) -> Option<String> {
    let s = match normalize_codec(codec).as_str() {
        "aac" => match profile.map(|p| p.to_ascii_lowercase()).as_deref() {
            Some("he-aac") => "mp4a.40.5",
            Some("he-aacv2") => "mp4a.40.29",
            _ => "mp4a.40.2",
        },
        "mp3" => "mp4a.40.34",
        "ac3" => "ac-3",
        "eac3" => "ec-3",
        "opus" => "opus",
        "flac" => "fLaC",
        "alac" => "alac",
        _ => return None,
    };
    Some(s.to_string())
}

/// The `CODECS` value of a variant: every present track must be describable.
pub fn attribute(video: Option<Option<String>>, audio: Option<Option<String>>) -> Option<String> {
    let mut parts = Vec::new();
    for track in [video, audio].into_iter().flatten() {
        parts.push(track?);
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

/// The fourCC hls.js groups levels by (`avc1`, `hvc1`, `mp4a`, …): variants of one session must
/// share it for every track, or hls.js ABR never switches between them.
pub fn family(codec_string: &str) -> &str {
    &codec_string[..codec_string.len().min(4)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_from_probe() {
        let s = |p, l| copied_video("h264", p, l, Some(8), 1920, 1080).unwrap();
        assert_eq!(s(Some("High"), Some(40)), "avc1.640028");
        assert_eq!(s(Some("High"), Some(41)), "avc1.640029");
        assert_eq!(s(Some("Main"), Some(31)), "avc1.4D001F");
        assert_eq!(s(Some("Constrained Baseline"), Some(30)), "avc1.42E01E");
        assert_eq!(s(Some("High 10"), Some(51)), "avc1.6E0033");
        // no level in the probe: from the picture size
        assert_eq!(s(Some("High"), None), "avc1.640028");
        assert_eq!(s(None, Some(-99)), "avc1.640028");
    }

    #[test]
    fn hevc_from_probe() {
        let s = |p, l, d| copied_video("hevc", p, l, d, 1920, 1038).unwrap();
        assert_eq!(s(Some("Main 10"), Some(120), Some(10)), "hvc1.2.4.L120.B0");
        assert_eq!(s(Some("Main"), Some(153), Some(8)), "hvc1.1.6.L153.B0");
        assert_eq!(s(None, None, Some(10)), "hvc1.2.4.L120.B0");
        assert_eq!(
            copied_video("h265", Some("Main"), None, None, 3840, 2160).unwrap(),
            "hvc1.1.6.L150.B0"
        );
    }

    #[test]
    fn other_video() {
        assert_eq!(
            copied_video("av1", Some("Main"), Some(8), Some(10), 1920, 1080).as_deref(),
            Some("av01.0.08M.10")
        );
        assert_eq!(copied_video("av1", None, None, None, 1920, 1080), None);
        assert_eq!(
            copied_video("mpeg2video", None, Some(8), None, 720, 576),
            None
        );
    }

    #[test]
    fn encoded() {
        assert_eq!(
            encoded_video("h264", 1280, 692, false).as_deref(),
            Some("avc1.64001F")
        );
        assert_eq!(
            encoded_video("h264", 1920, 1080, false).as_deref(),
            Some("avc1.640028")
        );
        assert_eq!(
            encoded_video("hevc", 854, 462, true).as_deref(),
            Some("hvc1.2.4.L93.B0")
        );
        assert_eq!(
            encoded_video("hevc", 1920, 1080, false).as_deref(),
            Some("hvc1.1.6.L120.B0")
        );
        assert_eq!(encoded_video("vp9", 1280, 720, false), None);
    }

    #[test]
    fn audio_strings() {
        assert_eq!(audio("aac", Some("LC")).as_deref(), Some("mp4a.40.2"));
        assert_eq!(audio("aac", Some("HE-AAC")).as_deref(), Some("mp4a.40.5"));
        assert_eq!(audio("aac", None).as_deref(), Some("mp4a.40.2"));
        assert_eq!(audio("e-ac-3", None).as_deref(), Some("ec-3"));
        assert_eq!(audio("ac3", None).as_deref(), Some("ac-3"));
        assert_eq!(audio("opus", None).as_deref(), Some("opus"));
        assert_eq!(audio("flac", None).as_deref(), Some("fLaC"));
        assert_eq!(audio("mp3", None).as_deref(), Some("mp4a.40.34"));
        assert_eq!(audio("dts", None), None);
    }

    #[test]
    fn attribute_needs_every_track() {
        let v = Some(Some("avc1.640028".to_string()));
        let a = Some(Some("mp4a.40.2".to_string()));
        assert_eq!(
            attribute(v.clone(), a.clone()).as_deref(),
            Some("avc1.640028,mp4a.40.2")
        );
        assert_eq!(attribute(v.clone(), None).as_deref(), Some("avc1.640028"));
        assert_eq!(attribute(v, Some(None)), None);
        assert_eq!(attribute(None, None), None);
    }

    #[test]
    fn families_match_hls_js_codec_sets() {
        assert_eq!(family("hvc1.2.4.L120.B0"), "hvc1");
        assert_eq!(family("hvc1.1.6.L93.B0"), "hvc1");
        assert_eq!(family("mp4a.40.2"), "mp4a");
        assert_eq!(family("ec-3"), "ec-3");
    }
}

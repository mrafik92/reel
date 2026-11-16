//! Pure ffmpeg command-line builder for HLS fMP4 output.

use crate::decision::{AudioAction, PlaybackDecision, VideoAction};
use crate::hwaccel::HwAccel;
use crate::model::Hdr;
use std::path::Path;

/// File names inside a session directory.
pub const INIT_NAME: &str = "init.mp4";
pub const SEGMENT_PATTERN: &str = "seg_%05d.m4s";
/// Copy-mode output: one file per source GOP, concatenated into playlist segments by the session.
pub const GOP_PATTERN: &str = "gop_%05d.m4s";
/// mov muxer options for HLS fMP4 segments: absolute timestamps after restarts, no edit lists
/// (they differ per run and shift the timeline), negative CTS offsets instead.
pub const HLS_SEGMENT_OPTIONS: &str = "movflags=+frag_discont+negative_cts_offsets:use_editlist=0";
pub const FFMPEG_PLAYLIST: &str = "ffmpeg.m3u8";

pub fn segment_name(n: u32) -> String {
    format!("seg_{n:05}.m4s")
}

pub fn gop_name(n: u32) -> String {
    format!("gop_{n:05}.m4s")
}

/// Parse `gop_00012.m4s` → 12.
pub fn parse_gop_name(name: &str) -> Option<u32> {
    name.strip_prefix("gop_")?
        .strip_suffix(".m4s")?
        .parse()
        .ok()
}

/// Parse `seg_00012.m4s` → 12.
pub fn parse_segment_name(name: &str) -> Option<u32> {
    name.strip_prefix("seg_")?
        .strip_suffix(".m4s")?
        .parse()
        .ok()
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnIn {
    /// Ordinal among the *subtitle* streams of the input (for `0:s:N` / `si=N`).
    pub ordinal: u32,
    pub image: bool,
    /// External text subtitle file to burn instead of an embedded stream.
    pub external_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BuildParams<'a> {
    pub input: &'a str,
    pub decision: &'a PlaybackDecision,
    pub hwaccel: &'a HwAccel,
    /// Source video codec (normalised); used for `-tag:v hvc1` on HEVC copy.
    pub source_video_codec: Option<&'a str>,
    pub source_hdr: Hdr,
    pub burn_in: Option<BurnIn>,
    pub start_segment: u32,
    pub segment_secs: u32,
    pub out_dir: &'a Path,
}

/// User-Agent for remote (provider) inputs.
pub const REMOTE_UA: &str = "VLC/3.0.20 LibVLC/3.0.20";

/// Is the input a network URL (remote VOD) rather than a local file?
pub fn is_remote(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

fn backslash_escape(s: &str, special: &[char]) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if special.contains(&c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// Escape a value (file name) for `filter=option=VALUE` inside a `-vf`/`-lavfi` graph.
///
/// ffmpeg parses twice (see "Quoting and escaping" in ffmpeg-utils): first the filter's option
/// string, where `\`, `'` and `:` are special, then the filtergraph description, where `\`, `'`,
/// `[`, `]`, `,` and `;` are special. The value is escaped for the first level, then the result for
/// the second.
pub fn escape_filter_value(s: &str) -> String {
    let level1 = backslash_escape(s, &['\\', '\'', ':']);
    backslash_escape(&level1, &['\\', '\'', '[', ']', ',', ';'])
}

/// `-protocol_whitelist` for an input: local files only, or plain http to the loopback upstream
/// proxy (never https/tls/crypto/concat/subfile/data…).
pub fn protocol_whitelist(remote: bool) -> &'static str {
    if remote { "http,tcp" } else { "file" }
}

const SW_TONEMAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// Whether the input should be hardware-decoded for this backend/decision.
fn hw_decode(hw: &HwAccel, burn: bool, tonemap: bool) -> bool {
    match hw {
        HwAccel::Vaapi { .. } => !burn,
        HwAccel::Qsv { .. } => !burn && !tonemap,
        _ => false,
    }
}

/// Seconds past a keyframe at which to seek so ffmpeg lands exactly on that keyframe (ffmpeg
/// seeks ~0.13s early when the stream has B-frames, otherwise to the last keyframe <= ts).
pub const KEYFRAME_SEEK_MARGIN: f64 = 0.25;

/// Fixed-grid output (encode, or copy when no keyframe plan is available).
pub fn build_args(p: &BuildParams<'_>) -> Vec<String> {
    build_inner(p, None)
}

/// Copy-mode output cut at every source keyframe (`gop_%05d.m4s`, numbered by keyframe index).
/// `start_segment` refers to `plan` segments; ffmpeg is started at that segment's keyframe.
pub fn build_args_with_plan(p: &BuildParams<'_>, plan: &crate::plan::SegmentPlan) -> Vec<String> {
    build_inner(p, Some(plan))
}

fn build_inner(p: &BuildParams<'_>, plan: Option<&crate::plan::SegmentPlan>) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    push(&[
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "warning",
        "-y",
    ]);

    let encode = match &p.decision.video {
        VideoAction::Encode {
            codec,
            width,
            height,
            bitrate_kbps,
            tonemap,
        } => Some((codec.as_str(), *width, *height, *bitrate_kbps, *tonemap)),
        _ => None,
    };
    let burn = p.burn_in.is_some();
    let tonemap = encode.is_some_and(|e| e.4);
    let hwdec = encode.is_some() && hw_decode(p.hwaccel, burn, tonemap);

    if encode.is_some() {
        match p.hwaccel {
            HwAccel::Vaapi { device, .. } => {
                push(&[
                    "-init_hw_device",
                    &format!("vaapi=va:{device}"),
                    "-filter_hw_device",
                    "va",
                ]);
                if hwdec {
                    push(&[
                        "-hwaccel",
                        "vaapi",
                        "-hwaccel_device",
                        "va",
                        "-hwaccel_output_format",
                        "vaapi",
                    ]);
                }
            }
            HwAccel::Qsv { device } => {
                push(&[
                    "-init_hw_device",
                    &format!("vaapi=va:{device}"),
                    "-init_hw_device",
                    "qsv=qs@va",
                    "-filter_hw_device",
                    "qs",
                ]);
                if hwdec {
                    push(&[
                        "-hwaccel",
                        "qsv",
                        "-hwaccel_device",
                        "qs",
                        "-hwaccel_output_format",
                        "qsv",
                    ]);
                }
            }
            _ => {}
        }
    }

    let remote = is_remote(p.input);
    if remote {
        // loopback upstream proxy: it supplies the provider URL/credentials/user agent; ffmpeg
        // only needs to survive dropped connections
        push(&[
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
        ]);
    }
    if p.start_segment > 0 {
        let t = match plan {
            Some(pl) => format!("{:.3}", pl.start(p.start_segment) + KEYFRAME_SEEK_MARGIN),
            None => (p.start_segment * p.segment_secs).to_string(),
        };
        if matches!(p.decision.video, VideoAction::Copy) {
            // do not trim decoded audio up to the (margin-shifted) seek time: audio must start
            // at the same keyframe the copied video starts at
            push(&["-noaccurate_seek"]);
        }
        push(&["-ss", &t]);
    }
    push(&[
        "-copyts",
        "-start_at_zero",
        "-protocol_whitelist",
        protocol_whitelist(remote),
        "-i",
        p.input,
    ]);

    // ---- stream mapping + filters ----
    let mut complex: Option<String> = None;
    let mut simple_vf: Option<String> = None;
    if let Some(vi) = p.decision.video_index {
        if let Some((_, w, h, _, tm)) = encode {
            let (pre, post) = filter_chains(p, hwdec, w, h, tm);
            match &p.burn_in {
                Some(b) if b.image => {
                    let pre = if pre.is_empty() {
                        "null".to_string()
                    } else {
                        pre.join(",")
                    };
                    let post = if post.is_empty() {
                        "null".to_string()
                    } else {
                        post.join(",")
                    };
                    complex = Some(format!(
                        "[0:{vi}]{pre}[v0];[v0][0:s:{}]overlay=eof_action=pass[v1];[v1]{post}[vout]",
                        b.ordinal
                    ));
                }
                Some(b) => {
                    let mut chain = pre;
                    let file = b.external_path.as_deref().unwrap_or(p.input);
                    let mut sub = format!("subtitles=filename={}", escape_filter_value(file));
                    if b.external_path.is_none() {
                        sub.push_str(&format!(":si={}", b.ordinal));
                    }
                    chain.push(sub);
                    chain.extend(post);
                    simple_vf = Some(chain.join(","));
                }
                None => {
                    let mut chain = pre;
                    chain.extend(post);
                    simple_vf = Some(chain.join(","));
                }
            }
        }
        if complex.is_some() {
            push(&[
                "-filter_complex",
                complex.as_deref().unwrap_or_default(),
                "-map",
                "[vout]",
            ]);
        } else {
            push(&["-map", &format!("0:{vi}")]);
        }
    }
    if let Some(ai) = p.decision.audio_index {
        push(&["-map", &format!("0:{ai}")]);
    }
    push(&["-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1"]);

    // ---- video codec ----
    match (&p.decision.video, encode) {
        (VideoAction::Copy, _) => {
            push(&["-c:v", "copy"]);
            if p.source_video_codec == Some("hevc") {
                push(&["-tag:v", "hvc1"]);
            }
        }
        (_, Some((codec, _, _, br, _))) => {
            if let Some(vf) = &simple_vf {
                push(&["-vf", vf]);
            }
            let enc = p.hwaccel.video_encoder(codec);
            push(&["-c:v", enc]);
            // fixed-function encoder when it passed the startup probe (#113)
            if p.hwaccel.low_power().for_codec(codec) {
                push(&["-low_power", "1"]);
            }
            // H.264 profile only; the HEVC encoders pick Main/Main10 from the input
            let profile: &[&str] = if enc.starts_with("hevc") {
                &[]
            } else {
                &["-profile:v", "high"]
            };
            match p.hwaccel {
                HwAccel::Software => push(&[
                    "-preset",
                    "veryfast",
                    "-profile:v",
                    "high",
                    "-sc_threshold",
                    "0",
                ]),
                HwAccel::Nvenc => {
                    push(&["-preset", "p4"]);
                    push(profile);
                }
                HwAccel::VideoToolbox => {
                    push(profile);
                    push(&["-realtime", "0"]);
                }
                HwAccel::Vaapi { .. } => {
                    push(profile);
                    push(&["-rc_mode", "VBR"]);
                }
                HwAccel::Qsv { .. } => {
                    push(profile);
                    push(&["-preset", "veryfast"]);
                }
            }
            push(&[
                "-b:v",
                &format!("{br}k"),
                "-maxrate",
                &format!("{}k", br + br / 5),
                "-bufsize",
                &format!("{}k", br * 2),
            ]);
            push(&[
                "-force_key_frames",
                &format!(
                    "expr:if(isnan(prev_forced_t),gte(t,0),gte(t,prev_forced_t+{}))",
                    p.segment_secs
                ),
            ]);
        }
        _ => {}
    }

    // ---- audio codec ----
    match &p.decision.audio {
        AudioAction::Copy => push(&["-c:a", "copy"]),
        AudioAction::Encode {
            codec,
            channels,
            bitrate_kbps,
        } => {
            push(&[
                "-c:a",
                codec,
                "-b:a",
                &format!("{bitrate_kbps}k"),
                "-ac",
                &channels.to_string(),
            ]);
        }
        AudioAction::None => {}
    }

    // ---- HLS fMP4 output ----
    let dir = p.out_dir;
    let (hls_time, pattern, start_number) = match plan {
        // cut at every keyframe; the session concatenates GOP files per plan segment
        Some(pl) => (
            "0.001".to_string(),
            GOP_PATTERN,
            pl.first_gop(p.start_segment),
        ),
        None => (p.segment_secs.to_string(), SEGMENT_PATTERN, p.start_segment),
    };
    let seg = dir.join(pattern);
    push(&[
        "-f",
        "hls",
        "-hls_time",
        &hls_time,
        "-hls_playlist_type",
        "event",
        "-hls_list_size",
        "0",
        "-hls_segment_type",
        "fmp4",
        "-hls_fmp4_init_filename",
        INIT_NAME,
        "-hls_segment_filename",
        &seg.to_string_lossy(),
        "-hls_flags",
        "independent_segments+temp_file",
        // keep absolute timestamps (tfdt) when ffmpeg is restarted mid-file
        "-hls_segment_options",
        HLS_SEGMENT_OPTIONS,
        "-start_number",
        &start_number.to_string(),
    ]);
    a.push(dir.join(FFMPEG_PLAYLIST).to_string_lossy().into_owned());
    a
}

/// Returns (pre-overlay chain, post chain). For text burn-in the subtitles filter sits between them.
fn filter_chains(
    p: &BuildParams<'_>,
    hwdec: bool,
    w: u32,
    h: u32,
    tonemap: bool,
) -> (Vec<String>, Vec<String>) {
    let _ = p.source_hdr;
    let mut pre: Vec<String> = Vec::new();
    let mut post: Vec<String> = Vec::new();
    match p.hwaccel {
        HwAccel::Vaapi { .. } if hwdec => {
            if tonemap {
                pre.push("tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709".into());
            }
            pre.push(format!("scale_vaapi=w={w}:h={h}:format=nv12"));
        }
        HwAccel::Qsv { .. } if hwdec => {
            pre.push(format!("scale_qsv=w={w}:h={h}:format=nv12"));
        }
        HwAccel::Vaapi { .. } => {
            if tonemap {
                pre.push(SW_TONEMAP.into());
            }
            post.push("format=nv12".into());
            post.push("hwupload".into());
            post.push(format!("scale_vaapi=w={w}:h={h}:format=nv12"));
        }
        HwAccel::Qsv { .. } => {
            if tonemap {
                pre.push(SW_TONEMAP.into());
            }
            post.push("format=nv12".into());
            post.push("hwupload=extra_hw_frames=64".into());
            post.push(format!("scale_qsv=w={w}:h={h}"));
        }
        _ => {
            if tonemap {
                pre.push(SW_TONEMAP.into());
            }
            post.push(format!("scale={w}:{h}"));
            post.push("format=yuv420p".into());
        }
    }
    (pre, post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{PlaybackMode, SubtitleAction};

    fn dec(video: VideoAction, audio: AudioAction) -> PlaybackDecision {
        PlaybackDecision {
            mode: PlaybackMode::Transcode,
            video,
            audio,
            subtitle: SubtitleAction::None,
            reasons: vec![],
            video_index: Some(0),
            audio_index: Some(1),
            subtitle_index: None,
        }
    }

    fn enc(tonemap: bool) -> VideoAction {
        VideoAction::Encode {
            codec: "h264".into(),
            width: 1280,
            height: 720,
            bitrate_kbps: 4000,
            tonemap,
        }
    }

    fn aac() -> AudioAction {
        AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 160,
        }
    }

    fn params<'a>(d: &'a PlaybackDecision, hw: &'a HwAccel, start: u32) -> BuildParams<'a> {
        BuildParams {
            input: "/m/a b.mkv",
            decision: d,
            hwaccel: hw,
            source_video_codec: Some("h264"),
            source_hdr: Hdr::None,
            burn_in: None,
            start_segment: start,
            segment_secs: 4,
            out_dir: Path::new("/tmp/o"),
        }
    }

    const HLS_TAIL: &str = "-f hls -hls_time 4 -hls_playlist_type event -hls_list_size 0 -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_filename /tmp/o/seg_%05d.m4s -hls_flags independent_segments+temp_file -hls_segment_options movflags=+frag_discont+negative_cts_offsets:use_editlist=0";
    const HEAD: &str = "-hide_banner -nostdin -nostats -loglevel warning -y";
    const KF: &str =
        "-force_key_frames expr:if(isnan(prev_forced_t),gte(t,0),gte(t,prev_forced_t+4))";

    fn s(args: Vec<String>) -> String {
        args.join(" ")
    }

    #[test]
    fn remux_copy_copy() {
        let d = dec(VideoAction::Copy, AudioAction::Copy);
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert_eq!(
            got,
            format!(
                "{HEAD} -copyts -start_at_zero -protocol_whitelist file -i /m/a b.mkv -map 0:0 -map 0:1 -sn -dn -map_metadata -1 -map_chapters -1 -c:v copy -c:a copy {HLS_TAIL} -start_number 0 /tmp/o/ffmpeg.m3u8"
            )
        );
    }

    #[test]
    fn hevc_copy_gets_hvc1_tag() {
        let d = dec(VideoAction::Copy, AudioAction::Copy);
        let mut p = params(&d, &HwAccel::Software, 0);
        p.source_video_codec = Some("hevc");
        assert!(s(build_args(&p)).contains("-c:v copy -tag:v hvc1"));
    }

    #[test]
    fn seek_start_number_and_ss_before_input() {
        let d = dec(VideoAction::Copy, aac());
        let got = s(build_args(&params(&d, &HwAccel::Software, 2)));
        assert!(
            got.contains("-ss 8 -copyts -start_at_zero -protocol_whitelist file -i /m/a b.mkv")
        );
        assert!(got.contains("-start_number 2 /tmp/o/ffmpeg.m3u8"));
        assert!(got.contains("-c:a aac -b:a 160k -ac 2"));
    }

    #[test]
    fn software_transcode() {
        let d = dec(enc(false), aac());
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert_eq!(
            got,
            format!(
                "{HEAD} -copyts -start_at_zero -protocol_whitelist file -i /m/a b.mkv -map 0:0 -map 0:1 -sn -dn -map_metadata -1 -map_chapters -1 -vf scale=1280:720,format=yuv420p -c:v libx264 -preset veryfast -profile:v high -sc_threshold 0 -b:v 4000k -maxrate 4800k -bufsize 8000k {KF} -c:a aac -b:a 160k -ac 2 {HLS_TAIL} -start_number 0 /tmp/o/ffmpeg.m3u8"
            )
        );
    }

    #[test]
    fn software_tonemap_chain() {
        let d = dec(enc(true), aac());
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert!(got.contains("-vf zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,scale=1280:720,format=yuv420p"));
    }

    #[test]
    fn vaapi_hw_decode_and_tonemap() {
        let hw = HwAccel::vaapi("/dev/dri/renderD128");
        let d = dec(enc(true), aac());
        let got = s(build_args(&params(&d, &hw, 0)));
        assert!(got.starts_with(&format!(
            "{HEAD} -init_hw_device vaapi=va:/dev/dri/renderD128 -filter_hw_device va -hwaccel vaapi -hwaccel_device va -hwaccel_output_format vaapi -copyts"
        )));
        assert!(got.contains("-vf tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709,scale_vaapi=w=1280:h=720:format=nv12 -c:v h264_vaapi -profile:v high -rc_mode VBR -b:v 4000k -maxrate 4800k -bufsize 8000k"));
    }

    #[test]
    fn vaapi_burn_in_text_uses_hwupload() {
        let hw = HwAccel::vaapi("/dev/dri/renderD128");
        let d = dec(enc(false), aac());
        let mut p = params(&d, &hw, 0);
        p.burn_in = Some(BurnIn {
            ordinal: 1,
            image: false,
            external_path: None,
        });
        let got = s(build_args(&p));
        assert!(!got.contains("-hwaccel vaapi"));
        assert!(got.contains(
            "-vf subtitles=filename=/m/a b.mkv:si=1,format=nv12,hwupload,scale_vaapi=w=1280:h=720:format=nv12 -c:v h264_vaapi"
        ));
    }

    fn vaapi_lp(h264: bool, hevc: bool) -> HwAccel {
        HwAccel::Vaapi {
            device: "/dev/dri/renderD128".into(),
            low_power: crate::hwaccel::LowPower { h264, hevc },
        }
    }

    fn enc_codec(codec: &str) -> VideoAction {
        VideoAction::Encode {
            codec: codec.into(),
            width: 1280,
            height: 720,
            bitrate_kbps: 4000,
            tonemap: false,
        }
    }

    #[test]
    fn vaapi_low_power_h264() {
        let d = dec(enc(false), aac());
        let got = s(build_args(&params(&d, &vaapi_lp(true, false), 0)));
        assert!(got.contains(
            "-vf scale_vaapi=w=1280:h=720:format=nv12 -c:v h264_vaapi -low_power 1 -profile:v high -rc_mode VBR -b:v 4000k -maxrate 4800k -bufsize 8000k"
        ), "{got}");
        // only one low_power flag, and the hardware pipeline is unchanged
        assert_eq!(got.matches("-low_power").count(), 1);
        assert!(got.contains("-hwaccel vaapi -hwaccel_device va -hwaccel_output_format vaapi"));
    }

    #[test]
    fn vaapi_without_low_power_support_uses_the_regular_encoder() {
        // HEVC-only LP must not switch the H.264 encoder to low power
        let d = dec(enc(false), aac());
        for hw in [vaapi_lp(false, false), vaapi_lp(false, true)] {
            let got = s(build_args(&params(&d, &hw, 0)));
            assert!(!got.contains("-low_power"), "{got}");
            assert!(got.contains("-c:v h264_vaapi -profile:v high -rc_mode VBR -b:v 4000k"));
        }
    }

    #[test]
    fn vaapi_hevc_output_low_power_and_fallback() {
        let d = dec(enc_codec("hevc"), aac());
        let got = s(build_args(&params(&d, &vaapi_lp(true, true), 0)));
        assert!(
            got.contains("-c:v hevc_vaapi -low_power 1 -rc_mode VBR -b:v 4000k"),
            "{got}"
        );
        assert!(!got.contains("-profile:v high"), "{got}");
        let got = s(build_args(&params(&d, &vaapi_lp(true, false), 0)));
        assert!(
            got.contains("-c:v hevc_vaapi -rc_mode VBR -b:v 4000k"),
            "{got}"
        );
        // no HEVC in software: H.264 with its profile
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert!(
            got.contains("-c:v libx264 -preset veryfast -profile:v high"),
            "{got}"
        );
    }

    #[test]
    fn low_power_never_applies_to_copy() {
        let d = dec(VideoAction::Copy, aac());
        let got = s(build_args(&params(&d, &vaapi_lp(true, true), 0)));
        assert!(
            !got.contains("-low_power") && !got.contains("-init_hw_device"),
            "{got}"
        );
    }

    #[test]
    fn qsv_chains() {
        let hw = HwAccel::Qsv {
            device: "/dev/dri/renderD128".into(),
        };
        let d = dec(enc(false), aac());
        let got = s(build_args(&params(&d, &hw, 0)));
        assert!(got.contains("-init_hw_device qsv=qs@va -filter_hw_device qs -hwaccel qsv -hwaccel_device qs -hwaccel_output_format qsv"));
        assert!(got.contains("-vf scale_qsv=w=1280:h=720:format=nv12 -c:v h264_qsv"));
        // tonemap forces software decode + upload
        let d = dec(enc(true), aac());
        let got = s(build_args(&params(&d, &hw, 0)));
        assert!(!got.contains("-hwaccel qsv"));
        assert!(got.contains("format=yuv420p,format=nv12,hwupload=extra_hw_frames=64,scale_qsv=w=1280:h=720 -c:v h264_qsv"));
    }

    #[test]
    fn nvenc_and_videotoolbox() {
        let d = dec(enc(false), aac());
        let got = s(build_args(&params(&d, &HwAccel::Nvenc, 0)));
        assert!(got.contains("-c:v h264_nvenc -preset p4 -profile:v high -b:v 4000k"));
        let got = s(build_args(&params(&d, &HwAccel::VideoToolbox, 0)));
        assert!(got.contains("-c:v h264_videotoolbox -profile:v high -realtime 0 -b:v 4000k"));
    }

    #[test]
    fn image_subtitle_overlay_uses_filter_complex() {
        let d = dec(enc(false), aac());
        let mut p = params(&d, &HwAccel::Software, 0);
        p.burn_in = Some(BurnIn {
            ordinal: 2,
            image: true,
            external_path: None,
        });
        let got = s(build_args(&p));
        assert!(got.contains(
            "-filter_complex [0:0]null[v0];[v0][0:s:2]overlay=eof_action=pass[v1];[v1]scale=1280:720,format=yuv420p[vout] -map [vout] -map 0:1"
        ));
        assert!(!got.contains("-vf "));
    }

    #[test]
    fn external_text_subtitle_burn_in_escapes() {
        let d = dec(enc(false), aac());
        let mut p = params(&d, &HwAccel::Software, 0);
        p.burn_in = Some(BurnIn {
            ordinal: 0,
            image: false,
            external_path: Some("/s/it's, [a].srt".into()),
        });
        let got = s(build_args(&p));
        // two-level escaping: \ ' : (option parser) then \ ' [ ] , ; (filtergraph parser)
        assert!(
            got.contains(r"-vf subtitles=filename=/s/it\\\'s\, \[a\].srt,scale=1280:720"),
            "{got}"
        );
    }

    #[test]
    fn eac3_audio_args_and_no_audio() {
        let d = dec(
            VideoAction::Copy,
            AudioAction::Encode {
                codec: "eac3".into(),
                channels: 6,
                bitrate_kbps: 640,
            },
        );
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert!(got.contains("-c:v copy -c:a eac3 -b:a 640k -ac 6 -f hls"));
        let mut d = dec(VideoAction::Copy, AudioAction::None);
        d.audio_index = None;
        let got = s(build_args(&params(&d, &HwAccel::Software, 0)));
        assert!(!got.contains("-c:a"));
        assert!(!got.contains("-map 0:1"));
    }

    #[test]
    fn copy_plan_cuts_at_keyframes_and_seeks_to_exact_gop() {
        let kfs: Vec<f64> = (0..20).map(|i| i as f64 * 1.48).collect();
        let plan = crate::plan::SegmentPlan::from_keyframes(&kfs, 29.6, 4);
        let d = dec(VideoAction::Copy, AudioAction::Copy);
        let got = s(build_args_with_plan(
            &params(&d, &HwAccel::Software, 2),
            &plan,
        ));
        // segment 2 starts at kf #6 (8.88s)
        assert!(got.contains("-ss 9.130 -copyts"), "{got}");
        assert!(got.contains("-hls_time 0.001"));
        assert!(got.contains("-hls_segment_filename /tmp/o/gop_%05d.m4s"));
        assert!(got.contains("-start_number 6 /tmp/o/ffmpeg.m3u8"), "{got}");
        let got0 = s(build_args_with_plan(
            &params(&d, &HwAccel::Software, 0),
            &plan,
        ));
        assert!(!got0.contains("-ss "));
        assert!(got0.contains("-start_number 0 "));
    }

    #[test]
    fn segment_names() {
        assert_eq!(segment_name(7), "seg_00007.m4s");
        assert_eq!(parse_segment_name("seg_00012.m4s"), Some(12));
        assert_eq!(parse_segment_name("init.mp4"), None);
    }
}

use reel_playback::decision::*;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;

fn file(path: &str, container: &str, kbps: i64) -> FileInfo {
    FileInfo {
        id: 1,
        path: path.into(),
        container: Some(container.into()),
        duration_s: Some(600.0),
        bitrate: Some(kbps * 1000),
    }
}

fn vid(codec: &str, w: i64, h: i64) -> StreamInfo {
    let mut s = StreamInfo::new(0, StreamKind::Video, codec);
    s.width = Some(w);
    s.height = Some(h);
    s.bit_depth = Some(8);
    s
}

fn aud(idx: i64, codec: &str, ch: i64, default: bool) -> StreamInfo {
    let mut s = StreamInfo::new(idx, StreamKind::Audio, codec);
    s.channels = Some(ch);
    s.default = default;
    s
}

fn sub(idx: i64, codec: &str) -> StreamInfo {
    StreamInfo::new(idx, StreamKind::Subtitle, codec)
}

struct Case {
    f: FileInfo,
    streams: Vec<StreamInfo>,
    audio: Option<i64>,
    sub: Option<i64>,
    profile: &'static str,
    cap: Option<u32>,
}

fn run(c: &Case) -> PlaybackDecision {
    let profile = DeviceProfile::preset(c.profile).unwrap();
    decide(&DecisionInput {
        file: &c.f,
        streams: &c.streams,
        audio_index: c.audio,
        subtitle_index: c.sub,
        profile: &profile,
        max_bitrate_kbps: c.cap,
    })
}

fn case(
    path: &str,
    container: &str,
    kbps: i64,
    streams: Vec<StreamInfo>,
    profile: &'static str,
) -> Case {
    Case {
        f: file(path, container, kbps),
        streams,
        audio: None,
        sub: None,
        profile,
        cap: None,
    }
}

fn mkv_h264_aac(profile: &'static str) -> Case {
    case(
        "/m/a.mkv",
        "matroska,webm",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "aac", 2, true)],
        profile,
    )
}

fn reasons(d: &PlaybackDecision) -> String {
    d.reasons.join(" | ")
}

#[test]
fn mkv_h264_aac_on_chrome_remuxes() {
    let d = run(&mkv_h264_aac("web_chrome"));
    assert_eq!(d.mode, PlaybackMode::Remux);
    assert_eq!(d.video, VideoAction::Copy);
    assert_eq!(d.audio, AudioAction::Copy);
    assert!(reasons(&d).contains("container mkv"));
}

#[test]
fn mp4_h264_aac_on_chrome_direct() {
    let c = case(
        "/m/a.mp4",
        "mov,mp4,m4a,3gp,3g2,mj2",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "aac", 2, true)],
        "web_chrome",
    );
    assert_eq!(run(&c).mode, PlaybackMode::DirectPlay);
}

#[test]
fn matroska_webm_format_name_with_mkv_extension_is_not_webm() {
    // ffprobe reports "matroska,webm" for .mkv; Chrome must not direct-play that as webm.
    let d = run(&mkv_h264_aac("web_chrome"));
    assert_ne!(d.mode, PlaybackMode::DirectPlay);
}

#[test]
fn hevc_hdr10_on_webos_direct() {
    let mut v = vid("hevc", 3840, 2160);
    v.hdr = Hdr::Hdr10;
    v.bit_depth = Some(10);
    v.profile = Some("Main 10".into());
    v.level = Some(153);
    let c = case(
        "/m/a.mkv",
        "matroska,webm",
        40000,
        vec![v, aud(1, "eac3", 6, true)],
        "webos",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::DirectPlay, "{}", reasons(&d));
}

#[test]
fn dts_on_web_audio_transcode() {
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "dts", 6, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::AudioTranscode);
    assert_eq!(d.video, VideoAction::Copy);
    assert_eq!(
        d.audio,
        AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 160
        }
    );
}

#[test]
fn dts_51_on_webos_becomes_eac3_51() {
    let c = case(
        "/m/a.mkv",
        "matroska,webm",
        20000,
        vec![vid("h264", 1920, 1080), aud(1, "dts", 6, true)],
        "webos",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::AudioTranscode);
    assert_eq!(
        d.audio,
        AudioAction::Encode {
            codec: "eac3".into(),
            channels: 6,
            bitrate_kbps: 640
        }
    );
}

#[test]
fn eac3_surround_on_chrome_downmixed_to_aac_stereo() {
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "eac3", 6, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::AudioTranscode);
    assert!(
        matches!(d.audio, AudioAction::Encode { ref codec, channels: 2, .. } if codec == "aac")
    );
}

#[test]
fn aac_surround_on_chrome_downmix() {
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "aac", 6, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::AudioTranscode);
    assert!(reasons(&d).contains("channels"));
}

#[test]
fn pgs_on_web_burns_in_with_full_transcode() {
    let mut c = mkv_h264_aac("web_chrome");
    c.streams.push(sub(2, "hdmv_pgs_subtitle"));
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert_eq!(d.subtitle, SubtitleAction::BurnIn);
    assert!(matches!(
        d.video,
        VideoAction::Encode { tonemap: false, .. }
    ));
    assert_eq!(d.subtitle_index, Some(2));
}

#[test]
fn pgs_on_exoplayer_is_native_direct() {
    let mut c = mkv_h264_aac("android_exoplayer");
    c.streams.push(sub(2, "hdmv_pgs_subtitle"));
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::DirectPlay);
    assert_eq!(d.subtitle, SubtitleAction::Native);
}

#[test]
fn pgs_on_exoplayer_with_remux_still_burns() {
    // DTS forces non-direct play; image subs cannot live in HLS → burn-in.
    let mut c = case(
        "/m/a.mkv",
        "matroska,webm",
        6000,
        vec![vid("h264", 1920, 1080), aud(1, "dts", 6, true)],
        "android_exoplayer",
    );
    c.streams.push(sub(2, "hdmv_pgs_subtitle"));
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(d.subtitle, SubtitleAction::BurnIn);
    assert_eq!(d.mode, PlaybackMode::Transcode);
}

#[test]
fn srt_on_chrome_is_sidecar_vtt() {
    let mut c = mkv_h264_aac("web_chrome");
    c.streams.push(sub(2, "subrip"));
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Remux);
    assert_eq!(
        d.subtitle,
        SubtitleAction::Sidecar {
            format: "vtt".into()
        }
    );
}

#[test]
fn ass_on_chrome_is_sidecar_and_srt_on_exoplayer_native() {
    let mut c = mkv_h264_aac("web_chrome");
    c.streams.push(sub(2, "ass"));
    c.sub = Some(2);
    assert_eq!(
        run(&c).subtitle,
        SubtitleAction::Sidecar {
            format: "vtt".into()
        }
    );
    let mut c = mkv_h264_aac("android_exoplayer");
    c.streams.push(sub(2, "subrip"));
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(
        (d.mode, d.subtitle),
        (PlaybackMode::DirectPlay, SubtitleAction::Native)
    );
}

#[test]
fn external_srt_is_always_sidecar() {
    let mut c = mkv_h264_aac("android_exoplayer");
    let mut s = sub(2, "subrip");
    s.external_path = Some("/m/a.en.srt".into());
    c.streams.push(s);
    c.sub = Some(2);
    assert_eq!(
        run(&c).subtitle,
        SubtitleAction::Sidecar {
            format: "vtt".into()
        }
    );
}

#[test]
fn uhd_over_bitrate_cap_downscales() {
    let mut v = vid("hevc", 3840, 2160);
    v.bit_depth = Some(10);
    let mut c = case(
        "/m/a.mkv",
        "matroska,webm",
        25000,
        vec![v, aud(1, "aac", 2, true)],
        "webos",
    );
    c.cap = Some(8000);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    match d.video.clone() {
        VideoAction::Encode {
            codec,
            width,
            height,
            bitrate_kbps,
            tonemap,
        } => {
            assert_eq!(codec, "h264");
            assert_eq!((width, height), (1920, 1080));
            assert!((5000..=8000).contains(&bitrate_kbps), "{bitrate_kbps}");
            assert!(!tonemap);
        }
        other => panic!("{other:?}"),
    }
    assert!(reasons(&d).contains("exceeds cap"));
}

#[test]
fn profile_cap_and_user_cap_min_wins() {
    let mut c = case(
        "/m/a.mp4",
        "mov,mp4",
        8000,
        vec![vid("h264", 1920, 1080), aud(1, "aac", 2, true)],
        "web_chrome",
    );
    assert_eq!(run(&c).mode, PlaybackMode::DirectPlay);
    c.cap = Some(4000);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    match d.video.clone() {
        VideoAction::Encode {
            height,
            bitrate_kbps,
            ..
        } => {
            assert_eq!(height, 720);
            assert!(bitrate_kbps < 4000);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn hdr_on_sdr_client_tonemaps() {
    let mut v = vid("hevc", 3840, 2160);
    v.hdr = Hdr::Hdr10;
    v.bit_depth = Some(10);
    let c = case(
        "/m/a.mkv",
        "matroska,webm",
        20000,
        vec![v, aud(1, "aac", 2, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert!(
        matches!(d.video, VideoAction::Encode { tonemap: true, .. }),
        "{:?}",
        d.video
    );
}

#[test]
fn dolby_vision_on_tizen_tonemaps_but_webos_direct() {
    let mk = |p| {
        let mut v = vid("hevc", 3840, 2160);
        v.hdr = Hdr::Dv;
        v.bit_depth = Some(10);
        case(
            "/m/a.mkv",
            "matroska,webm",
            30000,
            vec![v, aud(1, "eac3", 6, true)],
            p,
        )
    };
    let d = run(&mk("tizen"));
    assert!(matches!(d.video, VideoAction::Encode { tonemap: true, .. }));
    assert_eq!(run(&mk("webos")).mode, PlaybackMode::DirectPlay);
}

#[test]
fn hevc_sdr_on_chrome_transcodes_to_h264() {
    let mut v = vid("hevc", 1920, 1080);
    v.bit_depth = Some(8);
    let c = case(
        "/m/a.mkv",
        "matroska,webm",
        8000,
        vec![v, aud(1, "aac", 2, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert!(
        matches!(d.video, VideoAction::Encode { ref codec, tonemap: false, .. } if codec == "h264")
    );
    assert_eq!(d.audio, AudioAction::Copy);
}

#[test]
fn h264_10bit_and_high_level_transcode() {
    let mut v = vid("h264", 1920, 1080);
    v.bit_depth = Some(10);
    v.profile = Some("High 10".into());
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        8000,
        vec![v, aud(1, "aac", 2, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert!(reasons(&d).contains("bit depth 10"));

    let mut v = vid("h264", 1920, 1080);
    v.level = Some(62);
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        8000,
        vec![v, aud(1, "aac", 2, true)],
        "web_chrome",
    );
    assert!(reasons(&run(&c)).contains("level 62"));
}

#[test]
fn resolution_above_client_max_downscales() {
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        8000,
        vec![vid("h264", 7680, 4320), aud(1, "aac", 2, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert!(matches!(
        d.video,
        VideoAction::Encode {
            width: 3840,
            height: 2160,
            ..
        }
    ));
}

#[test]
fn alternate_audio_track_needs_remux_on_mse_only() {
    let mk = |p| {
        let mut c = case(
            "/m/a.mp4",
            "mov,mp4",
            6000,
            vec![
                vid("h264", 1920, 1080),
                aud(1, "aac", 2, true),
                aud(2, "aac", 2, false),
            ],
            p,
        );
        c.audio = Some(2);
        c
    };
    let d = run(&mk("web_chrome"));
    assert_eq!(d.mode, PlaybackMode::Remux);
    assert_eq!(d.audio_index, Some(2));
    assert_eq!(run(&mk("webos")).mode, PlaybackMode::DirectPlay);
}

#[test]
fn unknown_audio_index_falls_back_to_default() {
    let mut c = mkv_h264_aac("web_chrome");
    c.audio = Some(9);
    let d = run(&c);
    assert_eq!(d.audio_index, Some(1));
    assert!(reasons(&d).contains("not found"));
}

#[test]
fn burn_in_plus_hdr_encode_tonemaps() {
    let mut v = vid("hevc", 3840, 2160);
    v.hdr = Hdr::Hdr10;
    v.bit_depth = Some(10);
    let mut c = case(
        "/m/a.mkv",
        "matroska,webm",
        20000,
        vec![v, aud(1, "aac", 2, true), sub(2, "hdmv_pgs_subtitle")],
        "web_safari",
    );
    c.sub = Some(2);
    let d = run(&c);
    assert_eq!(d.mode, PlaybackMode::Transcode);
    assert!(matches!(d.video, VideoAction::Encode { tonemap: true, .. }));
}

#[test]
fn safari_mkv_hevc_remuxes_and_audio_only_file() {
    let mut v = vid("hevc", 1920, 1080);
    v.bit_depth = Some(10);
    let c = case(
        "/m/a.mkv",
        "matroska,webm",
        8000,
        vec![v, aud(1, "eac3", 6, true)],
        "web_safari",
    );
    assert_eq!(run(&c).mode, PlaybackMode::Remux);
    let c = case(
        "/m/a.mp4",
        "mov,mp4",
        300,
        vec![aud(0, "aac", 2, true)],
        "web_chrome",
    );
    let d = run(&c);
    assert_eq!(
        (d.mode, d.video.clone()),
        (PlaybackMode::DirectPlay, VideoAction::None)
    );
}

#[test]
fn unknown_container_remuxes() {
    let mut c = mkv_h264_aac("web_chrome");
    c.f.container = None;
    c.f.path = "/m/a.bin".into();
    assert_eq!(run(&c).mode, PlaybackMode::Remux);
}

#[test]
fn db_mode_mapping_and_presets() {
    assert_eq!(PlaybackMode::DirectPlay.db_mode(), "direct");
    assert_eq!(PlaybackMode::Remux.db_mode(), "remux");
    assert_eq!(PlaybackMode::AudioTranscode.db_mode(), "transcode");
    for n in DeviceProfile::preset_names() {
        let p = DeviceProfile::preset(n).unwrap();
        assert_eq!(&p.name, n);
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<DeviceProfile>(&j).unwrap(), p);
    }
    let d = run(&mkv_h264_aac("web_chrome"));
    let j = serde_json::to_value(&d).unwrap();
    assert_eq!(j["mode"], "remux");
    assert_eq!(j["video"]["type"], "copy");
}

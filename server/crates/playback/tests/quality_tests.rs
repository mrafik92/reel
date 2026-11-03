use reel_playback::decision::*;
use reel_playback::model::*;
use reel_playback::profile::DeviceProfile;
use reel_playback::quality::*;

fn file(path: &str, container: &str, kbps: i64) -> FileInfo {
    FileInfo {
        id: 1,
        path: path.into(),
        container: Some(container.into()),
        duration_s: Some(5629.0),
        bitrate: Some(kbps * 1000),
    }
}

fn vid(codec: &str, w: i64, h: i64) -> StreamInfo {
    let mut s = StreamInfo::new(0, StreamKind::Video, codec);
    s.width = Some(w);
    s.height = Some(h);
    s.bit_depth = Some(8);
    s.profile = Some("High".into());
    s
}

fn aud(codec: &str, ch: i64) -> StreamInfo {
    let mut s = StreamInfo::new(1, StreamKind::Audio, codec);
    s.channels = Some(ch);
    s.default = true;
    s
}

/// Resident Evil: MKV, H.264 1920x800, AAC 5.1 (EAC3 would be audio-transcoded), 9.5 Mbps.
fn re_streams() -> Vec<StreamInfo> {
    vec![vid("h264", 1920, 800), aud("aac", 6)]
}

fn run(
    f: &FileInfo,
    streams: &[StreamInfo],
    profile: &str,
    quality: Quality,
    hint: Option<u32>,
    user: Option<u32>,
) -> QualityDecision {
    let p = DeviceProfile::preset(profile).unwrap();
    decide_quality(
        &DecisionInput {
            file: f,
            streams,
            audio_index: None,
            subtitle_index: None,
            profile: &p,
            max_bitrate_kbps: user,
        },
        &QualityRequest {
            quality,
            max_bitrate_kbps: hint,
        },
    )
}

fn enc(d: &PlaybackDecision) -> (String, u32, u32, u32, bool) {
    match &d.video {
        VideoAction::Encode {
            codec,
            width,
            height,
            bitrate_kbps,
            tonemap,
        } => (codec.clone(), *width, *height, *bitrate_kbps, *tonemap),
        other => panic!("expected encode, got {other:?}"),
    }
}

fn opt(q: &QualityDecision, id: Quality) -> &QualityOption {
    q.options.iter().find(|o| o.id == id).unwrap()
}

#[test]
fn cap_above_source_keeps_original() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    for hint in [None, Some(12_000), Some(9_600)] {
        let q = run(&f, &re_streams(), "web_chrome", Quality::Auto, hint, None);
        assert_eq!(q.decision.mode, PlaybackMode::Remux, "hint {hint:?}");
        assert_eq!(q.quality, Quality::Original);
        assert_eq!(q.bitrate_kbps, 9500);
    }
}

#[test]
fn auto_picks_highest_rung_that_fits_the_cap() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    // 12 Mbps needed, 3.5 Mbps available (the live DERP relay case): 480p (2000+128)
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(3500),
        None,
    );
    assert_eq!(q.decision.mode, PlaybackMode::Transcode);
    assert_eq!(q.quality, Quality::P480);
    let (codec, w, h, br, tm) = enc(&q.decision);
    assert_eq!(
        (codec.as_str(), w, h, br, tm),
        ("h264", 854, 356, 2000, false)
    );
    assert_eq!(
        q.decision.audio,
        AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 128
        }
    );
    assert_eq!(q.bitrate_kbps, 2128);
    // 6 Mbps → 720p, 9 Mbps → 1080p rung (no downscale of a 1920x800 scope picture)
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(6000),
        None,
    );
    assert_eq!(
        (
            q.quality,
            enc(&q.decision).1,
            enc(&q.decision).2,
            enc(&q.decision).3
        ),
        (Quality::P720, 1280, 532, 4000)
    );
    assert_eq!(
        q.decision.audio,
        AudioAction::Encode {
            codec: "aac".into(),
            channels: 2,
            bitrate_kbps: 160
        }
    );
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(9000),
        None,
    );
    assert_eq!(
        (
            q.quality,
            enc(&q.decision).1,
            enc(&q.decision).2,
            enc(&q.decision).3
        ),
        (Quality::P1080, 1920, 800, 8000)
    );
    assert_eq!(
        q.decision.audio,
        AudioAction::Copy,
        "1080p keeps the original audio"
    );
    assert!(
        q.decision
            .reasons
            .iter()
            .any(|r| r.contains("adaptive quality 1080p")),
        "{:?}",
        q.decision.reasons
    );
}

#[test]
fn user_limit_is_a_hard_cap_even_for_original_and_beats_a_larger_hint() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Original,
        None,
        Some(5000),
    );
    assert_eq!(q.quality, Quality::P720);
    assert!(q.bitrate_kbps <= 5000);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(20_000),
        Some(5000),
    );
    assert_eq!(q.quality, Quality::P720);
    // original is not offered under the limit, rungs that exceed it neither (480p stays as floor)
    assert!(!opt(&q, Quality::Original).available);
    assert!(!opt(&q, Quality::P1080).available);
    assert!(opt(&q, Quality::P720).available && opt(&q, Quality::P480).available);
    // a user limit above the source does not change anything
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Original,
        None,
        Some(50_000),
    );
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Remux, Quality::Original)
    );
}

#[test]
fn explicit_original_ignores_the_client_hint() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Original,
        Some(1000),
        None,
    );
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Remux, Quality::Original)
    );
}

#[test]
fn explicit_rungs_transcode_even_with_plenty_of_bandwidth() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::P720,
        Some(50_000),
        None,
    );
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Transcode, Quality::P720)
    );
    assert_eq!(enc(&q.decision).3, 4000);
    // explicit rung that does not fit the cap steps down
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::P1080,
        Some(3000),
        None,
    );
    assert_eq!(q.quality, Quality::P480);
}

#[test]
fn never_upscales() {
    let f = file("/m/a.mkv", "matroska,webm", 4000);
    let s = vec![vid("h264", 1280, 720), aud("aac", 2)];
    let q = run(&f, &s, "web_chrome", Quality::P1080, None, None);
    assert!(!opt(&q, Quality::P1080).available);
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Remux, Quality::Original),
        "unavailable rung falls back"
    );
    let q = run(&f, &s, "web_chrome", Quality::P480, None, None);
    assert_eq!(q.quality, Quality::P480);
    assert_eq!((enc(&q.decision).1, enc(&q.decision).2), (852, 480));
    // a 480p source offers only original
    let small = vec![vid("h264", 854, 480), aud("aac", 2)];
    let f = file("/m/s.mkv", "matroska,webm", 1500);
    let q = run(&f, &small, "web_chrome", Quality::Auto, Some(500), None);
    assert!(
        q.options
            .iter()
            .filter(|o| o.available && o.id != Quality::Auto && o.id != Quality::Original)
            .count()
            <= 1
    );
    let (_, w, h, _, _) = enc(&q.decision);
    assert!(w <= 854 && h <= 480, "{w}x{h}");
}

#[test]
fn rung_that_changes_nothing_is_not_offered() {
    // 3 Mbps 1080p: the 1080p rung (8 Mbps) would only inflate; 720p still shrinks the picture
    let f = file("/m/a.mkv", "matroska,webm", 3000);
    let s = vec![vid("h264", 1920, 1080), aud("aac", 2)];
    let q = run(&f, &s, "web_chrome", Quality::P1080, None, None);
    assert!(!opt(&q, Quality::P1080).available);
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Remux, Quality::Original),
        "no point re-encoding"
    );
    // 720p still shrinks the picture; its video bitrate never exceeds the source's
    let q = run(&f, &s, "web_chrome", Quality::P720, None, None);
    assert_eq!(q.quality, Quality::P720);
    assert_eq!(enc(&q.decision).3, 3000);
}

#[test]
fn codec_incompatible_with_low_cap_uses_the_ladder() {
    let mut v = vid("hevc", 1920, 1080);
    v.profile = Some("Main 10".into());
    v.bit_depth = Some(8);
    let f = file("/m/h.mkv", "matroska,webm", 9000);
    let s = vec![v, aud("aac", 2)];
    // plenty of bandwidth: compat transcode as before, not a rung
    let q = run(&f, &s, "web_chrome", Quality::Auto, Some(30_000), None);
    assert_eq!(q.decision.mode, PlaybackMode::Transcode);
    assert!(q.bitrate_kbps <= 30_000);
    // 2.5 Mbps: 480p rung
    let q = run(&f, &s, "web_chrome", Quality::Auto, Some(2500), None);
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::Transcode, Quality::P480)
    );
    assert!(q.bitrate_kbps <= 2500, "{}", q.bitrate_kbps);
    // user limit applies likewise
    let q = run(&f, &s, "web_chrome", Quality::Auto, None, Some(4500));
    assert_eq!(q.quality, Quality::P720);
}

#[test]
fn cap_below_the_lowest_rung_scales_the_bitrate_down() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(800),
        None,
    );
    assert_eq!(q.quality, Quality::P480);
    let br = enc(&q.decision).3;
    assert!((300..=800 - 128).contains(&br), "{br}");
    assert!(q.bitrate_kbps <= 800);
}

#[test]
fn direct_play_becomes_transcode_when_bandwidth_is_short() {
    let f = file("/m/a.mp4", "mov,mp4", 9500);
    let q = run(&f, &re_streams(), "web_chrome", Quality::Auto, None, None);
    assert_eq!(
        (q.decision.mode, q.quality),
        (PlaybackMode::DirectPlay, Quality::Original)
    );
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(5000),
        None,
    );
    assert_eq!(q.decision.mode, PlaybackMode::Transcode);
    assert!(
        !q.decision
            .reasons
            .iter()
            .any(|r| r.starts_with("client supports container"))
    );
}

#[test]
fn hdr_source_is_tonemapped_on_the_ladder() {
    let mut v = vid("hevc", 3840, 2160);
    v.hdr = Hdr::Hdr10;
    v.bit_depth = Some(10);
    let f = file("/m/u.mkv", "matroska,webm", 40_000);
    let q = run(
        &f,
        &[v, aud("eac3", 6)],
        "webos",
        Quality::Auto,
        Some(7000),
        None,
    );
    assert_eq!(q.decision.mode, PlaybackMode::Transcode);
    let (codec, w, h, _, tm) = enc(&q.decision);
    assert!(tm && codec == "h264");
    assert_eq!((w, h), (1280, 720));
    assert_eq!(q.quality, Quality::P720);
}

#[test]
fn burn_in_subtitles_survive_the_rung_and_native_ones_become_sidecar() {
    let mut s = re_streams();
    s.push(StreamInfo::new(
        2,
        StreamKind::Subtitle,
        "hdmv_pgs_subtitle",
    ));
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let p = DeviceProfile::preset("android_exoplayer").unwrap();
    let mk = |sub: i64, s: &[StreamInfo]| {
        decide_quality(
            &DecisionInput {
                file: &f,
                streams: s,
                audio_index: None,
                subtitle_index: Some(sub),
                profile: &p,
                max_bitrate_kbps: None,
            },
            &QualityRequest {
                quality: Quality::P720,
                max_bitrate_kbps: None,
            },
        )
    };
    // exoplayer renders PGS natively in direct play; in an HLS transcode it must be burned in
    let q = mk(2, &s);
    assert_eq!(q.decision.subtitle, SubtitleAction::BurnIn);
    s.push(StreamInfo::new(3, StreamKind::Subtitle, "subrip"));
    let q = mk(3, &s);
    assert_eq!(
        q.decision.subtitle,
        SubtitleAction::Sidecar {
            format: "vtt".into()
        }
    );
}

#[test]
fn options_list_shape() {
    let f = file("/m/a.mkv", "matroska,webm", 9500);
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(3500),
        None,
    );
    let ids: Vec<_> = q.options.iter().map(|o| o.id.id()).collect();
    assert_eq!(ids, ["auto", "original", "1080p", "720p", "480p"]);
    assert_eq!(opt(&q, Quality::Auto).bitrate_kbps, q.bitrate_kbps);
    assert_eq!(opt(&q, Quality::Original).bitrate_kbps, 9500);
    assert!(opt(&q, Quality::Original).label.starts_with("Original"));
    assert!(
        q.options.iter().all(|o| o.available),
        "1920x800 source offers every rung and no limit applies"
    );
    let j = serde_json::to_value(&q.options[2]).unwrap();
    assert_eq!(j["id"], "1080p");
    assert_eq!(
        serde_json::from_str::<Quality>("\"720p\"").unwrap(),
        Quality::P720
    );
}

#[test]
fn audio_only_and_unknown_bitrate_files_stay_original() {
    let f = file("/m/a.mp4", "mov,mp4", 300);
    let q = run(
        &f,
        &[aud("aac", 2)],
        "web_chrome",
        Quality::Auto,
        Some(100),
        None,
    );
    assert_eq!(q.quality, Quality::Original);
    assert!(
        q.options
            .iter()
            .filter(|o| o.available)
            .all(|o| o.id == Quality::Auto || o.id == Quality::Original)
    );
    // unknown source bitrate: nothing to compare, original is kept
    let mut f = file("/m/a.mkv", "matroska,webm", 1);
    f.bitrate = None;
    let q = run(
        &f,
        &re_streams(),
        "web_chrome",
        Quality::Auto,
        Some(3000),
        None,
    );
    assert_eq!(q.quality, Quality::Original);
}

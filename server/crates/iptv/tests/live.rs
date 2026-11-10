mod common;

use std::{
    sync::{Arc, atomic::Ordering::SeqCst},
    time::{Duration, Instant},
};

use common::*;
use futures_util::StreamExt;
use reel_iptv::{
    ChannelHub, Error, HlsOptions, HubConfig, LiveManager, LiveMode,
    hls::{AudioMode, VideoMode},
    ts::parse_packet,
};

fn cfg() -> HubConfig {
    HubConfig {
        grace: Duration::from_millis(300),
        stall_after: Duration::from_secs(2),
        reap_interval: Duration::from_millis(40),
        ..Default::default()
    }
}

fn manager(res: Arc<MapResolver>, idle: Duration) -> (Arc<LiveManager>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let hub = ChannelHub::new(cfg(), res);
    hub.spawn_reaper();
    let live = LiveManager::new(hub, dir.path().join("hls"), idle);
    live.spawn_reaper();
    (live, dir)
}

#[tokio::test]
async fn hls_copy_is_shared_between_viewers_and_plays() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing; skipping");
        return;
    }
    let (_d, path) = temp_ts(6);
    let prov = Provider::start(&path, 6, 1.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "News", vec![cand(1, 10, prov.url("news"), Some(1))]);
    let (live, _dir) = manager(res, Duration::from_secs(30));

    let t0 = Instant::now();
    let a = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    let b = live
        .tune(2, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    assert_ne!(a.id, b.id);
    let hls = a.hls.as_ref().unwrap();
    assert_eq!(
        (hls.video, hls.audio),
        (VideoMode::Copy, AudioMode::Copy),
        "h264+aac is copied"
    );

    let index = live.hls_file(&a.id, "index.m3u8").await.unwrap();
    println!("HLS first playlist after {:?}", t0.elapsed());
    let text = std::fs::read_to_string(&index).unwrap();
    assert!(text.starts_with("#EXTM3U"), "{text}");
    assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4\""), "{text}");
    assert!(text.contains("#EXTINF"), "{text}");
    let seg = text
        .lines()
        .find(|l| l.starts_with("seg_"))
        .expect("a segment")
        .to_string();
    let seg_path = live.hls_file(&a.id, &seg).await.unwrap();
    assert!(std::fs::metadata(&seg_path).unwrap().len() > 500);
    assert!(
        std::fs::metadata(live.hls_file(&a.id, "init.mp4").await.unwrap())
            .unwrap()
            .len()
            > 100
    );
    // the second viewer sees the very same playlist/segments
    assert_eq!(live.hls_file(&b.id, "index.m3u8").await.unwrap(), index);
    // one provider connection, one ffmpeg for two HLS viewers
    assert_eq!(prov.counters("news").total.load(SeqCst), 1);
    assert_eq!(
        live.hub().metrics()[0].viewers,
        1,
        "the shared ffmpeg is the only hub viewer"
    );

    // the produced segments are valid fMP4 that ffprobe can read back
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&index)
        .output()
        .unwrap();
    let codecs = String::from_utf8_lossy(&probe.stdout).to_string();
    assert!(
        codecs.contains("h264") && codecs.contains("aac"),
        "{codecs}"
    );

    // names that ffmpeg doesn't write are refused
    assert!(matches!(
        live.hls_file(&a.id, "../etc/passwd").await,
        Err(Error::NotFound(_))
    ));
    assert!(live.hls_file("nope", "index.m3u8").await.is_err());
    live.shutdown().await;
}

#[tokio::test]
async fn hls_reencodes_audio_the_browser_cannot_play() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mp2.ts");
    make_ts_audio(&path, 6, "mp2");
    let prov = Provider::start(&path, 6, 1.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "Classic", vec![cand(1, 10, prov.url("classic"), None)]);
    let (live, _d) = manager(res, Duration::from_secs(30));
    let s = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    let h = s.hls.as_ref().unwrap();
    assert_eq!(
        (h.video, h.audio),
        (VideoMode::Copy, AudioMode::Aac),
        "mp2 → aac, video still copied"
    );
    let index = live.hls_file(&s.id, "index.m3u8").await.unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&index)
        .output()
        .unwrap();
    let codecs = String::from_utf8_lossy(&probe.stdout).to_string();
    assert!(
        codecs.contains("aac") && !codecs.contains("mp2"),
        "{codecs}"
    );
    live.shutdown().await;
}

#[tokio::test]
async fn passthrough_reserves_slot_at_tune_and_streams_ts_from_gop_start() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "One", vec![cand(1, 10, prov.url("one"), Some(1))]);
    res.add(2, "Two", vec![cand(2, 10, prov.url("two"), Some(1))]);
    let (live, _dir) = manager(res, Duration::from_millis(600));

    let a = live
        .tune(1, 1, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .unwrap();
    assert_eq!(a.mode, LiveMode::Passthrough);
    // user 1 has not even opened the player yet, but the single provider slot is reserved
    let err = live
        .tune(2, 2, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, Error::InUseByOthers { max: 1, .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("One"), "{err}");

    let mut st = Box::pin(live.ts_stream(&a.id).await.unwrap());
    let first = st.next().await.unwrap().unwrap();
    assert_eq!(first[0], 0x47);
    assert_eq!(
        parse_packet(&first).unwrap().pid,
        0,
        "stream starts with the PAT"
    );
    let mut got = first.len();
    while got < 20_000 {
        got += st.next().await.unwrap().unwrap().len();
    }
    assert_eq!(live.list()[0].connections, 1);
    // wrong mode → clear error
    assert!(live.hls_file(&a.id, "index.m3u8").await.is_err());
    drop(st);
    // an unconnected session is reaped after the idle timeout and its viewer released
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(live.session(&a.id).is_none(), "idle session dropped");
    assert!(live.hub().metrics().first().is_none_or(|m| m.viewers == 0));
    assert!(
        live.tune(2, 2, LiveMode::Passthrough, &HlsOptions::default())
            .await
            .is_ok(),
        "slot freed"
    );
    live.shutdown().await;
}

#[tokio::test]
async fn video_transcode_falls_back_to_software_when_the_hardware_encoder_fails() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mpeg2.ts");
    make_ts_codecs(&path, 6, "mpeg2video", "mp2");
    let prov = Provider::start(&path, 6, 1.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "SD", vec![cand(1, 10, prov.url("sd"), None)]);
    let (live, _d) = manager(res, Duration::from_secs(30));
    // a VAAPI device that does not exist: ffmpeg cannot open it
    live.set_hwaccel(reel_playback::HwAccel::Vaapi {
        device: "/dev/dri/renderD999".into(),
    });
    assert_eq!(live.hw_status(), ("vaapi", false));
    let s = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    let h = s.hls.as_ref().unwrap();
    assert_eq!(
        (h.video, h.audio),
        (VideoMode::Transcode, AudioMode::Aac),
        "MPEG-2 must be re-encoded"
    );
    assert_eq!(h.encoder, "software", "hardware failed → software fallback");
    assert_eq!(
        live.hw_status(),
        ("vaapi", false),
        "per-attempt fallback: one failure does not disable the hardware"
    );
    let index = live.hls_file(&s.id, "index.m3u8").await.unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&index)
        .output()
        .unwrap();
    let codecs = String::from_utf8_lossy(&probe.stdout).to_string();
    assert!(
        codecs.contains("h264") && codecs.contains("aac") && !codecs.contains("mpeg2"),
        "{codecs}"
    );
    live.shutdown().await;
}

#[tokio::test]
async fn direct_sessions_hold_a_provider_slot_until_they_stop_heartbeating() {
    let res = Arc::new(MapResolver::default());
    res.add(
        1,
        "One",
        vec![cand(
            1,
            10,
            "http://provider.invalid/live/one".into(),
            Some(1),
        )],
    );
    let (live, _dir) = manager(res, Duration::from_millis(600));

    let a = live
        .tune(1, 1, LiveMode::Direct, &HlsOptions::default())
        .await
        .unwrap();
    assert_eq!(a.mode, LiveMode::Direct);
    assert_eq!(
        a.direct_url.as_deref(),
        Some("http://provider.invalid/live/one")
    );
    assert!(live.hub().metrics().is_empty(), "no hub pipe is opened");
    assert!(
        live.session(&a.id).unwrap().direct_url.is_some(),
        "kept for the owner's tune response only"
    );
    // another user can't get a second connection on the same one-slot source
    let err = live
        .tune(2, 1, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, Error::InUseByOthers { max: 1, .. }),
        "{err:?}"
    );
    // heartbeats keep it alive well past the idle timeout
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(live.keepalive(&a.id));
    }
    assert!(live.session(&a.id).is_some());
    // no more heartbeats: the reaper closes it and the slot is free again
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(live.session(&a.id).is_none(), "timed out");
    assert!(!live.keepalive(&a.id), "tells the client to re-tune");
    live.tune(2, 1, LiveMode::Direct, &HlsOptions::default())
        .await
        .expect("the slot was released");
    // direct sessions have nothing to stream from Reel
    assert!(live.ts_stream("nope").await.is_err());
}

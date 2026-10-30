mod common;

use std::{
    sync::{Arc, atomic::Ordering::SeqCst},
    time::{Duration, Instant},
};

use common::*;
use reel_iptv::{
    ChannelHub, Error, HubConfig,
    ts::{PKT, TsAnalyzer, parse_packet},
};

fn fast_cfg() -> HubConfig {
    HubConfig {
        grace: Duration::from_millis(400),
        warm_ttl: Duration::from_secs(30),
        stall_after: Duration::from_millis(400),
        failover_after: 2,
        backoff_base: Duration::from_millis(30),
        backoff_max: Duration::from_millis(100),
        reap_interval: Duration::from_millis(40),
        ..Default::default()
    }
}

async fn read_bytes(sub: &mut reel_iptv::Subscription, n: usize) -> usize {
    let mut total = 0;
    let end = Instant::now() + Duration::from_secs(10);
    while total < n && Instant::now() < end {
        match tokio::time::timeout(Duration::from_secs(2), sub.next()).await {
            Ok(Some(b)) => total += b.len(),
            _ => break,
        }
    }
    total
}

#[test]
fn fixture_keyframes_and_stream_info() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg/libx264/aac missing; skipping");
        return;
    }
    let (_d, path) = temp_ts(8);
    let data = std::fs::read(&path).unwrap();
    assert_eq!(data.len() % PKT, 0);
    let mut an = TsAnalyzer::new();
    let mut keys = vec![];
    for (i, p) in data.chunks(PKT).enumerate() {
        if an.analyze(&bytes::Bytes::copy_from_slice(p)).keyframe {
            keys.push(i);
        }
    }
    // 25 fps, GOP 25, 8 s → keyframes at t=0..7 s
    assert_eq!(keys.len(), 8, "{keys:?}");
    let info = an.info.as_ref().unwrap();
    assert_eq!(info.video.map(|v| v.name()), Some("h264"));
    assert_eq!(info.audio.first().map(|a| a.1.name()), Some("aac"));
    assert_eq!(an.cc_errors, 0);
    assert_eq!(an.pts_discontinuities, 0);
    assert!(an.psi().is_some());
}

#[tokio::test]
async fn three_viewers_share_one_upstream_and_late_joiner_starts_instantly() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing; skipping");
        return;
    }
    let (_d, path) = temp_ts(8);
    let prov = Provider::start(&path, 8, 2.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "News", vec![cand(1, 10, prov.url("news"), Some(2))]);
    let hub = ChannelHub::new(fast_cfg(), res);
    hub.spawn_reaper();

    let mut a = hub.subscribe(1, Some(1)).await.unwrap();
    let mut b = hub.subscribe(1, Some(2)).await.unwrap();
    // viewers A and B start before any data exists: both wait for the first keyframe
    let (ra, rb) = tokio::join!(read_bytes(&mut a, 30_000), read_bytes(&mut b, 30_000));
    assert!(ra >= 30_000 && rb >= 30_000, "{ra} {rb}");

    // late joiner, several GOPs in
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t0 = Instant::now();
    let mut late = hub.subscribe(1, Some(3)).await.unwrap();
    let mut pieces = vec![late.next().await.unwrap()];
    let ttfb = t0.elapsed();
    pieces.push(late.next().await.unwrap());
    pieces.push(late.next().await.unwrap());
    println!("late-joiner time to first bytes: {:?}", ttfb);
    assert!(ttfb < Duration::from_millis(200), "ttfb {ttfb:?}");

    // first bytes: PAT, PMT, then a video access unit starting at a keyframe
    let p0 = parse_packet(&pieces[0]).unwrap();
    assert_eq!(p0.pid, 0, "PAT first");
    let info = late.stream_info().unwrap();
    let p1 = parse_packet(&pieces[1]).unwrap();
    assert_eq!(p1.pid, info.pmt_pid, "PMT second");
    let p2 = parse_packet(&pieces[2]).unwrap();
    assert_eq!(Some(p2.pid), info.video_pid);
    assert!(p2.pusi && p2.random_access, "keyframe-aligned start");

    // all of that over a single provider connection
    let c = prov.counters("news");
    assert_eq!(c.total.load(SeqCst), 1, "one upstream for 3 viewers");
    assert_eq!(c.max_active.load(SeqCst), 1);

    let m = &hub.metrics()[0];
    assert_eq!(m.viewers, 3);
    assert!(m.bitrate_kbps > 0, "{m:?}");
    assert!(m.ttfb_ms.is_some() && m.time_to_keyframe_ms.is_some());
    assert!(m.keyframes >= 3);
    assert_eq!(m.cc_errors, 0, "{m:?}");
    assert_eq!(m.video_codec.as_deref(), Some("h264"));
    println!(
        "metrics: bitrate {} kbps, ttfb {:?} ms, first keyframe {:?} ms",
        m.bitrate_kbps, m.ttfb_ms, m.time_to_keyframe_ms
    );

    // teardown: after the last viewer leaves + grace, the provider connection closes
    drop((a, b, late));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(hub.is_open(1), "still inside the grace period");
    let end = Instant::now() + Duration::from_secs(3);
    while hub.is_open(1) && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!hub.is_open(1));
    let end = Instant::now() + Duration::from_secs(3);
    while c.active.load(SeqCst) > 0 && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(c.active.load(SeqCst), 0, "upstream connection closed");
    // re-zap within grace would have reused the connection; after teardown it reconnects
    let mut again = hub.subscribe(1, Some(1)).await.unwrap();
    assert!(read_bytes(&mut again, 5_000).await >= 5_000);
    assert_eq!(c.total.load(SeqCst), 2);
    hub.shutdown();
}

#[tokio::test]
async fn zapping_back_within_grace_reuses_the_connection() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "A", vec![cand(1, 10, prov.url("a"), None)]);
    let mut cfg = fast_cfg();
    cfg.grace = Duration::from_secs(5);
    let hub = ChannelHub::new(cfg, res);
    hub.spawn_reaper();
    let mut s = hub.subscribe(1, Some(1)).await.unwrap();
    assert!(read_bytes(&mut s, 20_000).await >= 20_000);
    drop(s);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let t0 = Instant::now();
    let mut s2 = hub.subscribe(1, Some(1)).await.unwrap();
    let first = s2.next().await.unwrap();
    println!("re-zap within grace: first bytes after {:?}", t0.elapsed());
    assert!(t0.elapsed() < Duration::from_millis(50));
    assert_eq!(parse_packet(&first).unwrap().pid, 0);
    assert_eq!(prov.counters("a").total.load(SeqCst), 1);
    hub.shutdown();
}

#[tokio::test]
async fn connection_budget_evicts_warm_channels_and_refuses_when_all_watched() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    let res = Arc::new(MapResolver::default());
    // one provider account with a single connection, three channels
    for (id, n) in [(1, "one"), (2, "two"), (3, "three")] {
        res.add(
            id,
            &n.to_uppercase(),
            vec![cand(id, 7, prov.url(n), Some(1))],
        );
    }
    let mut cfg = fast_cfg();
    cfg.grace = Duration::from_secs(30); // unwatched pipes stay warm until evicted
    let hub = ChannelHub::new(cfg, res);
    hub.spawn_reaper();

    let mut s1 = hub.subscribe(1, Some(1)).await.unwrap();
    assert!(read_bytes(&mut s1, 10_000).await >= 10_000);
    // channel 2 while 1 is watched: no slot, clear error naming who holds it
    let err = hub
        .subscribe(2, Some(2))
        .await
        .err()
        .expect("budget exhausted");
    match &err {
        Error::BudgetExhausted {
            source_id,
            max,
            watching,
        } => {
            assert_eq!((*source_id, *max), (7, 1));
            assert_eq!(watching, &vec!["ONE".to_string()]);
        }
        e => panic!("{e:?}"),
    }
    assert!(err.to_string().contains("ONE"));
    assert!(!hub.is_open(2));

    // viewer leaves: channel 1 stays warm (grace 30 s) …
    drop(s1);
    assert!(hub.is_open(1));
    // … and is evicted when channel 2 needs the slot
    let mut s2 = hub.subscribe(2, Some(2)).await.unwrap();
    assert!(
        !hub.is_open(1),
        "least-recently-viewed warm channel evicted"
    );
    assert!(read_bytes(&mut s2, 10_000).await >= 10_000);
    let end = Instant::now() + Duration::from_secs(3);
    while prov.counters("one").active.load(SeqCst) > 0 && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(prov.counters("one").active.load(SeqCst), 0);
    assert_eq!(hub.connections_by_source().get(&7), Some(&1));

    // warming never evicts and never exceeds the limit
    assert!(!hub.warm(3, false).await.unwrap());
    assert!(!hub.is_open(3));
    hub.shutdown();
}

#[tokio::test]
async fn warm_channels_use_free_slots_and_pinning_survives_grace() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    let res = Arc::new(MapResolver::default());
    for (id, n) in [(1, "w1"), (2, "w2"), (3, "w3")] {
        res.add(id, n, vec![cand(id, 7, prov.url(n), Some(2))]);
    }
    let hub = ChannelHub::new(fast_cfg(), res);
    hub.spawn_reaper();
    assert!(hub.warm(1, true).await.unwrap(), "pinned favorite warmed");
    assert!(hub.warm(2, false).await.unwrap());
    assert!(!hub.warm(3, false).await.unwrap(), "limit of 2 reached");
    tokio::time::sleep(Duration::from_millis(900)).await; // > grace
    assert!(hub.is_open(1), "pinned stays");
    let ms = hub.metrics();
    assert!(ms.iter().find(|m| m.channel_id == 1).unwrap().pinned);
    // viewer on a pinned warm channel gets data immediately from the cached GOP
    let mut s = hub.subscribe(1, None).await.unwrap();
    let t0 = Instant::now();
    s.next().await.unwrap();
    assert!(t0.elapsed() < Duration::from_millis(100));
    hub.shutdown();
}

#[tokio::test]
async fn failover_to_alias_when_primary_stalls() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    prov.set("primary", Behavior::StallAfter(5_000));
    let res = Arc::new(MapResolver::default());
    res.add(
        1,
        "Main",
        vec![
            cand(1, 10, prov.url("primary"), None),
            cand(2, 20, prov.url("backup"), None),
        ],
    );
    let hub = ChannelHub::new(fast_cfg(), res);
    hub.spawn_reaper();
    let mut s = hub.subscribe(1, Some(1)).await.unwrap();
    // data keeps flowing across the failover (≈ 2 stalls × 400 ms + backoff)
    let got = read_bytes(&mut s, 60_000).await;
    assert!(got >= 60_000, "got {got}");
    let m = &hub.metrics()[0];
    assert_eq!(m.failovers, 1, "{m:?}");
    assert_eq!(m.active_candidate, 1);
    assert_eq!(
        m.source_id, 20,
        "budget accounting follows the alias' source"
    );
    assert!(m.reconnects >= 2);
    assert!(
        prov.counters("primary").total.load(SeqCst) >= 2,
        "primary retried before failover"
    );
    assert!(prov.counters("backup").total.load(SeqCst) >= 1);
    assert!(m.last_error.as_deref().unwrap().contains("stalled"));
    hub.shutdown();
}

#[tokio::test]
async fn http_errors_reconnect_with_backoff_and_recover() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 2.0).await;
    prov.set("flaky", Behavior::Status(503));
    let res = Arc::new(MapResolver::default());
    res.add(1, "Flaky", vec![cand(1, 10, prov.url("flaky"), None)]);
    let hub = ChannelHub::new(fast_cfg(), res);
    hub.spawn_reaper();
    let mut s = hub.subscribe(1, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(prov.counters("flaky").total.load(SeqCst) >= 2, "retries");
    assert!(
        hub.metrics()[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("503")
    );
    prov.set("flaky", Behavior::Normal);
    assert!(
        read_bytes(&mut s, 20_000).await >= 20_000,
        "recovers once the provider is back"
    );
    hub.shutdown();
}

#[tokio::test]
async fn slow_viewer_never_blocks_others_and_is_resynced() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, path) = temp_ts(4);
    let prov = Provider::start(&path, 4, 8.0).await; // fast provider
    let res = Arc::new(MapResolver::default());
    res.add(1, "Fast", vec![cand(1, 10, prov.url("fast"), None)]);
    let mut cfg = fast_cfg();
    cfg.ring_chunks = 16; // tiny ring so the slow reader lags quickly
    let hub = ChannelHub::new(cfg, res);
    hub.spawn_reaper();
    let mut slow = hub.subscribe(1, Some(1)).await.unwrap();
    let mut fast = hub.subscribe(1, Some(2)).await.unwrap();
    // `fast` keeps reading while `slow` sleeps for 1.5 s
    let reader = tokio::spawn(async move {
        let start = Instant::now();
        let mut n = 0usize;
        while start.elapsed() < Duration::from_millis(1500) {
            if let Ok(Some(b)) = tokio::time::timeout(Duration::from_millis(500), fast.next()).await
            {
                n += b.len();
            }
        }
        n
    });
    let fast_bytes = reader.await.unwrap();
    assert!(fast_bytes > 100_000, "fast viewer unaffected: {fast_bytes}");
    // slow viewer resumes at a keyframe boundary
    let first = slow.next().await.unwrap();
    assert_eq!(parse_packet(&first).unwrap().pid, 0, "resynced: PAT first");
    assert!(hub.metrics()[0].viewer_resyncs >= 1);
    hub.shutdown();
}

#[tokio::test]
async fn hls_providers_are_pulled_through_ffmpeg_into_the_same_pipeline() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    make_hls(dir.path(), 6);
    let addr = serve_hls(dir.path().to_path_buf()).await;
    let res = Arc::new(MapResolver::default());
    // detected by the `.m3u8` extension …
    res.add(
        1,
        "ByExt",
        vec![cand(1, 10, format!("http://{addr}/index.m3u8"), None)],
    );
    // … and by `Content-Type: application/vnd.apple.mpegurl` alone
    res.add(
        2,
        "ByType",
        vec![cand(2, 10, format!("http://{addr}/playlist"), None)],
    );
    let hub = ChannelHub::new(
        HubConfig {
            stall_after: Duration::from_secs(10),
            ..fast_cfg()
        },
        res,
    );
    hub.spawn_reaper();
    for id in [1, 2] {
        let mut sub = hub.subscribe(id, None).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(15), sub.next())
            .await
            .expect("first bytes")
            .unwrap();
        // the ffmpeg-produced MPEG-TS goes through the normal alignment + keyframe logic
        assert_eq!(
            parse_packet(&first).unwrap().pid,
            0,
            "channel {id}: starts with the PAT"
        );
        let got = read_bytes(&mut sub, 40_000).await;
        assert!(got >= 40_000, "channel {id}: {got}");
        let info = sub.stream_info().expect("PMT parsed");
        assert_eq!(info.video.map(|v| v.name()), Some("h264"));
        assert_eq!(info.audio.first().map(|a| a.1.name()), Some("aac"));
        let m = hub
            .metrics()
            .into_iter()
            .find(|m| m.channel_id == id)
            .unwrap();
        assert!(m.keyframes >= 1 && m.bytes_total > 40_000, "{m:?}");
    }
    // only http(s) is accepted as an upstream scheme
    let bad = Arc::new(MapResolver::default());
    bad.add(
        3,
        "File",
        vec![cand(3, 10, "file:///etc/passwd".into(), None)],
    );
    let hub2 = ChannelHub::new(fast_cfg(), bad);
    let _s = hub2.subscribe(3, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let m = &hub2.metrics()[0];
    assert!(m.last_error.as_deref().unwrap().contains("http"), "{m:?}");
    hub.shutdown();
    hub2.shutdown();
}

#[tokio::test]
async fn strict_mode_refuses_private_upstreams() {
    let res = Arc::new(MapResolver::default());
    res.add(
        1,
        "Lan",
        vec![cand(1, 10, "http://127.0.0.1:9/x.ts".into(), None)],
    );
    res.add(
        2,
        "Meta",
        vec![cand(
            2,
            10,
            "http://169.254.169.254/latest/meta-data".into(),
            None,
        )],
    );
    let hub = ChannelHub::new(
        HubConfig {
            allow_private_upstream: false,
            ..fast_cfg()
        },
        res,
    );
    for id in [1, 2] {
        let _s = hub.subscribe(id, None).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for m in hub.metrics() {
        assert!(
            m.last_error.as_deref().unwrap().contains("blocked"),
            "{m:?}"
        );
        assert_eq!(m.bytes_total, 0);
    }
    hub.shutdown();
}

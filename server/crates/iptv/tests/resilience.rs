//! Single-connection providers, hardware-fallback attribution.
mod common;

use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use common::*;
use reel_iptv::{ChannelHub, Error, HlsOptions, HubConfig, LiveManager, LiveMode};

/// A provider with ONE connection slot that is released with a lag: after a client disconnects
/// every new connection gets `503` for `lag`, like real Xtream panels.
struct Slots {
    data: Bytes,
    active: AtomicUsize,
    max_active: AtomicUsize,
    refused: AtomicUsize,
    busy_until: Mutex<Instant>,
    lag: Duration,
    always_503: Mutex<bool>,
}

struct Release(Arc<Slots>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, SeqCst);
        *self.0.busy_until.lock().unwrap() = Instant::now() + self.0.lag;
    }
}

async fn serve(State(s): State<Arc<Slots>>, AxPath(_n): AxPath<String>) -> Response {
    let blocked = *s.always_503.lock().unwrap()
        || s.active.load(SeqCst) > 0
        || Instant::now() < *s.busy_until.lock().unwrap();
    if blocked {
        s.refused.fetch_add(1, SeqCst);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let now = s.active.fetch_add(1, SeqCst) + 1;
    s.max_active.fetch_max(now, SeqCst);
    let guard = Release(s.clone());
    let data = s.data.clone();
    let stream = futures_util::stream::unfold(
        (
            guard,
            0usize,
            tokio::time::interval(Duration::from_millis(20)),
        ),
        move |(g, pos, mut tick)| {
            let data = data.clone();
            async move {
                tick.tick().await;
                let n = 1880usize.min(data.len() - pos);
                let c = data.slice(pos..pos + n);
                Some((
                    Ok::<_, std::io::Error>(c),
                    (g, (pos + n) % data.len(), tick),
                ))
            }
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp2t")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn slot_provider(file: &std::path::Path, lag: Duration) -> (Arc<Slots>, String) {
    let s = Arc::new(Slots {
        data: Bytes::from(std::fs::read(file).unwrap()),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        refused: AtomicUsize::new(0),
        busy_until: Mutex::new(Instant::now()),
        lag,
        always_503: Mutex::new(false),
    });
    let app = Router::new()
        .route("/live/{n}", get(serve))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (s, base)
}

fn cfg() -> HubConfig {
    HubConfig {
        grace: Duration::from_secs(5),
        warm_ttl: Duration::from_secs(30),
        stall_after: Duration::from_secs(3),
        reap_interval: Duration::from_millis(40),
        ..Default::default()
    }
}

fn resolver(base: &str, max: Option<u32>) -> Arc<MapResolver> {
    let res = Arc::new(MapResolver::default());
    for (id, name) in [(1, "A"), (2, "B"), (3, "C")] {
        res.add(
            id,
            name,
            vec![cand(id, 10, format!("{base}/live/{name}"), max)],
        );
    }
    res
}

#[tokio::test]
async fn switching_on_a_one_slot_provider_frees_the_old_upstream_first_then_retries_the_lag() {
    let (_d, ts) = temp_ts(6);
    let (prov, base) = slot_provider(&ts, Duration::from_secs(1)).await;
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    hub.spawn_reaper();

    let mut a = hub.subscribe(1, Some(7)).await.unwrap();
    a.wait_ready(Duration::from_secs(3)).await.unwrap();
    assert!(a.next().await.is_some());
    assert_eq!(prov.active.load(SeqCst), 1);
    drop(a); // the viewer leaves …

    // … and immediately zaps to B: no grace for A, no warm channel
    let t = Instant::now();
    let mut b = hub.subscribe(2, Some(7)).await.unwrap();
    assert!(
        !hub.is_open(1),
        "old channel was released before the new one opened"
    );
    b.wait_ready(Duration::from_secs(5)).await.unwrap();
    assert!(b.next().await.is_some());
    let took = t.elapsed();
    assert!(took < Duration::from_millis(3200), "{took:?}");
    assert!(
        took >= Duration::from_millis(700),
        "waited for the provider to release: {took:?}"
    );
    assert!(
        prov.refused.load(SeqCst) >= 1,
        "503s were retried, not given up on"
    );
    assert_eq!(
        prov.max_active.load(SeqCst),
        1,
        "never two upstream connections"
    );
    let open: Vec<i64> = hub.metrics().iter().map(|m| m.channel_id).collect();
    assert_eq!(open, [2], "no warm channel kept");
    hub.shutdown();
}

#[tokio::test]
async fn a_busy_provider_gives_a_clear_error_after_the_window_instead_of_spinning() {
    let (_d, ts) = temp_ts(6);
    let (prov, base) = slot_provider(&ts, Duration::from_secs(1)).await;
    *prov.always_503.lock().unwrap() = true;
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    let sub = hub.subscribe(3, Some(1)).await.unwrap();
    let t = Instant::now();
    let err = sub.wait_ready(Duration::from_secs(8)).await.unwrap_err();
    let took = t.elapsed();
    assert!(matches!(err, Error::Unavailable(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("provider refused") && msg.contains("503") && msg.contains("1 concurrent"),
        "{msg}"
    );
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(5),
        "{took:?}"
    );
    let tries = prov.refused.load(SeqCst);
    assert!(
        (8..=16).contains(&tries),
        "~every 250 ms for ~3 s, not a spin: {tries}"
    );
    assert!(
        !hub.is_open(3),
        "the failed pipe is gone; the next tune starts clean"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        prov.refused.load(SeqCst),
        tries,
        "no further attempts after giving up"
    );
    hub.shutdown();
}

#[tokio::test]
async fn one_slot_providers_never_get_warm_channels() {
    let (_d, ts) = temp_ts(6);
    let (prov, base) = slot_provider(&ts, Duration::from_millis(0)).await;
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    assert!(
        !hub.warm(1, true).await.unwrap(),
        "no warm/pinned favourites on a single slot"
    );
    assert!(hub.metrics().is_empty());
    assert_eq!(prov.refused.load(SeqCst) + prov.active.load(SeqCst), 0);
    // a provider with spare slots still warms
    let hub2 = ChannelHub::new(cfg(), resolver(&base, Some(3)));
    assert!(hub2.warm(1, false).await.unwrap());
    hub.shutdown();
    hub2.shutdown();
}

#[tokio::test]
async fn hls_channel_switch_on_a_one_slot_provider_releases_the_previous_session() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, ts) = temp_ts(6);
    let (prov, base) = slot_provider(&ts, Duration::from_secs(1)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    hub.spawn_reaper();
    let live = LiveManager::new(hub.clone(), dir.path().join("hls"), Duration::from_secs(30));
    live.spawn_reaper();

    let a = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    live.hls_file(&a.id, "index.m3u8").await.unwrap();
    // same user zaps; the previous HLS output still holds the only slot (viewer + packager)
    let t = Instant::now();
    let b = live
        .tune(1, 2, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    live.hls_file(&b.id, "index.m3u8").await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    assert!(
        live.session(&a.id).is_none(),
        "previous session was released"
    );
    assert_eq!(prov.max_active.load(SeqCst), 1);
    assert_eq!(
        hub.metrics()
            .iter()
            .map(|m| m.channel_id)
            .collect::<Vec<_>>(),
        [2]
    );

    // another user cannot take the slot from a live viewer: a clear 'in use' error
    let err = live
        .tune(2, 3, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InUseByOthers { max: 1, .. }),
        "{err:?}"
    );
    live.shutdown().await;
}

fn fake_ffmpeg(dir: &std::path::Path, hw_branch: &str) -> String {
    let p = dir.join("ffmpeg-fake");
    std::fs::write(
        &p,
        format!(
            "#!/bin/bash\ncase \"$*\" in\n  *vaapi*)\n    head -c 60000 > /dev/null\n    {hw_branch}\n    ;;\n  *) exec ffmpeg \"$@\" ;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.to_string_lossy().into_owned()
}

async fn mpeg2_live(
    ffmpeg: String,
) -> (
    Arc<LiveManager>,
    Provider,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("mpeg2.ts");
    make_ts_codecs(&path, 6, "mpeg2video", "mp2");
    let prov = Provider::start(&path, 6, 4.0).await;
    let res = Arc::new(MapResolver::default());
    res.add(1, "SD", vec![cand(1, 10, prov.url("sd"), None)]);
    let dir = tempfile::tempdir().unwrap();
    let hub = ChannelHub::new(
        HubConfig {
            ffmpeg_path: ffmpeg,
            stall_after: Duration::from_secs(3),
            ..Default::default()
        },
        res,
    );
    hub.spawn_reaper();
    let live = LiveManager::new(hub, dir.path().join("hls"), Duration::from_secs(30));
    live.set_hwaccel(reel_playback::HwAccel::vaapi("/dev/dri/renderD128"));
    (live, prov, dir, src)
}

#[tokio::test]
async fn a_vaapi_failure_with_input_flowing_falls_back_for_this_start_only() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ff = fake_ffmpeg(
        tmp.path(),
        "echo '[AVHWDeviceContext] Failed to initialise VAAPI connection: -1 (unknown libva error).' >&2; exit 1",
    );
    let (live, _prov, _d, _s) = mpeg2_live(ff).await;
    let s = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    let h = s.hls.as_ref().unwrap();
    assert_eq!(h.encoder, "software", "this start used libx264");
    assert_eq!(
        live.hw_status(),
        ("vaapi", false),
        "hardware is not written off after one failure"
    );
    live.hls_file(&s.id, "index.m3u8").await.unwrap();
    live.shutdown().await;
}

#[tokio::test]
async fn a_failure_that_is_not_the_hardwares_does_not_fall_back_or_flag_it() {
    if !have_ffmpeg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // ffmpeg dies on the stream itself (no hardware wording in stderr)
    let ff = fake_ffmpeg(
        tmp.path(),
        "echo 'Invalid data found when processing input' >&2; exit 1",
    );
    let (live, _prov, _d, _s) = mpeg2_live(ff).await;
    let err = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unavailable(_)), "{err:?}");
    assert!(err.to_string().contains("Invalid data"), "{err}");
    assert_eq!(
        live.hw_status(),
        ("vaapi", false),
        "GPU not blamed for a bad stream"
    );
    live.shutdown().await;
}

#[tokio::test]
async fn idle_sessions_release_the_slot_hls_after_no_requests_passthrough_on_disconnect() {
    if !have_ffmpeg() {
        return;
    }
    let (_d, ts) = temp_ts(6);
    let (prov, base) = slot_provider(&ts, Duration::from_millis(200)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    hub.spawn_reaper();
    // sessions idle out after 1 s here (production: HLS 15 s)
    let live = LiveManager::new(hub.clone(), dir.path().join("hls"), Duration::from_secs(1));
    live.spawn_reaper();

    // HLS: the player never asks for the playlist/segments
    let h = live
        .tune(1, 1, LiveMode::Hls, &HlsOptions::default())
        .await
        .unwrap();
    assert!(live.session(&h.id).is_some());
    let t = Instant::now();
    loop {
        if live.session(&h.id).is_none() && hub.metrics().is_empty() {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "slot was not released: {:?}",
            hub.metrics()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // another user can now tune without any takeover
    let p = live
        .tune(2, 2, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .unwrap();
    {
        use futures_util::StreamExt;
        let mut s = Box::pin(live.ts_stream(&p.id).await.unwrap());
        assert!(s.next().await.is_some());
        // the player disconnects (stream dropped) …
    }
    // … and the session + upstream go away shortly after
    let t = Instant::now();
    loop {
        if live.session(&p.id).is_none() && hub.metrics().is_empty() {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(6),
            "passthrough slot not released: {:?}",
            hub.metrics()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let again = live
        .tune(3, 1, LiveMode::Passthrough, &HlsOptions::default())
        .await;
    assert!(again.is_ok(), "{again:?}");
    assert_eq!(prov.max_active.load(SeqCst), 1);
    live.shutdown().await;
}

#[tokio::test]
async fn stopping_a_running_passthrough_stream_by_takeover_ends_it() {
    let (_d, ts) = temp_ts(6);
    let (_prov, base) = slot_provider(&ts, Duration::from_millis(100)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = ChannelHub::new(cfg(), resolver(&base, Some(1)));
    hub.spawn_reaper();
    let live = LiveManager::new(hub.clone(), dir.path().join("hls"), Duration::from_secs(30));
    let a = live
        .tune(1, 1, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .unwrap();
    use futures_util::StreamExt;
    let mut stream = Box::pin(live.ts_stream(&a.id).await.unwrap());
    assert!(stream.next().await.is_some());
    // the HTTP server polls the body in its own task
    let drain = tokio::spawn(async move { while stream.next().await.is_some() {} });
    // same user, other device, other channel: the old *running* stream is cut
    let b = live
        .tune(1, 2, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .is_ok(),
        "the old stream must end"
    );
    assert!(live.session(&a.id).is_none() && live.session(&b.id).is_some());
    // an admin takeover of another user's running stream
    let err = live
        .tune(2, 1, LiveMode::Passthrough, &HlsOptions::default())
        .await
        .unwrap_err();
    let Error::InUseByOthers { holders, .. } = err else {
        panic!("{err:?}")
    };
    assert_eq!(
        (holders[0].user_id, holders[0].channel_name.as_str()),
        (1, "B")
    );
    let c = live
        .tune_ex(2, 1, LiveMode::Passthrough, &HlsOptions::default(), true)
        .await
        .unwrap();
    assert!(live.session(&b.id).is_none() && live.session(&c.id).is_some());
    live.shutdown().await;
}

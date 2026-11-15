use std::time::Duration;

use reel_playback::{
    FileInfo, HwAccel, PlaybackMode, SessionConfig, SessionManager, StartRequest,
    decision::{AudioAction, PlaybackDecision, SubtitleAction, VideoAction},
};

fn req() -> StartRequest {
    StartRequest {
        user_id: 1,
        device_id: None,
        file: FileInfo {
            id: 1,
            path: "/nowhere.mp4".into(),
            container: Some("mp4".into()),
            duration_s: Some(10.0),
            bitrate: None,
        },
        streams: vec![],
        decision: PlaybackDecision {
            mode: PlaybackMode::DirectPlay,
            video: VideoAction::Copy,
            audio: AudioAction::Copy,
            subtitle: SubtitleAction::None,
            reasons: vec![],
            video_index: None,
            audio_index: None,
            subtitle_index: None,
        },
        start_position_s: 0.0,
        user_max_streams: None,
        user_max_transcodes: None,
        user_agent: None,
    }
}

/// Direct play keeps its own timeout whatever the player says: a paused VOD direct-play response
/// holds a provider connection (A25), so "paused" doesn't extend it.
#[tokio::test]
async fn a_paused_ping_does_not_extend_direct_play() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = SessionConfig::new(dir.path());
    cfg.direct_idle_timeout = Duration::from_millis(300);
    cfg.paused_idle_timeout = Duration::from_secs(3600);
    let m = SessionManager::new(cfg, HwAccel::Software);
    let id = m.create_session(req()).await.unwrap().id;
    m.update_progress(&id, 2.0, None, true).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(m.reap_idle().await, vec![id]);
}

/// A cast receiver fetching media (without progress pings from the sender) keeps the session alive.
#[tokio::test]
async fn media_requests_from_any_client_keep_a_session_alive() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = SessionConfig::new(dir.path());
    cfg.direct_idle_timeout = Duration::from_millis(400);
    let m = SessionManager::new(cfg, HwAccel::Software);
    let kept = m.create_session(req()).await.unwrap().id;
    let idle = m.create_session(req()).await.unwrap().id;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        m.touch_session(&kept);
        assert!(m.reap_idle().await.iter().all(|r| r != &kept));
    }
    assert!(m.session(&kept).await.is_ok(), "touched session survives");
    assert!(
        m.session(&idle).await.is_err(),
        "the untouched one was reaped"
    );
}

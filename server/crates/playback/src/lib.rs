//! Playback: device profiles, decision engine, HW accel detection, ffmpeg command builder,
//! transcode session manager, HLS serving and subtitle conversion.
//!
//! Typical wiring (API lane):
//! ```ignore
//! let hw = reel_playback::hwaccel::detect().await;                       // once at startup
//! let mgr = SessionManager::new(SessionConfig::new(data_dir), hw.selected.clone());
//! mgr.spawn_reaper();
//! let app = app.merge(reel_playback::router::stream_router(mgr.clone())); // /stream/{sid}/...
//! // POST /playback/{fileId}/decide:
//! let (file, streams) = model::load_file(&pool, id).await?.unwrap();
//! let d = decision::decide(&DecisionInput { file: &file, streams: &streams, audio_index, subtitle_index, profile: &profile, max_bitrate_kbps });
//! // if d.mode != DirectPlay: mgr.create_session(StartRequest { .. }).await  -> url /stream/{id}/master.m3u8
//! ```

pub mod decision;
pub mod error;
pub mod ffmpeg;
pub mod fmp4;
pub mod hwaccel;
pub mod keyindex;
pub mod langs;
pub mod model;
pub mod plan;
pub mod profile;
pub mod router;
pub mod select;
pub mod session;
pub mod subtitles;

pub use decision::{DecisionInput, PlaybackDecision, PlaybackMode, decide};
pub use error::PlaybackError;
pub use hwaccel::{HwAccel, HwAccelStatus, detect};
pub use model::{FileInfo, Hdr, StreamInfo, StreamKind};
pub use plan::SegmentPlan;
pub use profile::DeviceProfile;
pub use select::{Pick, PlaybackPrefs, Selected, SubtitleMode};
pub use session::{SessionConfig, SessionInfo, SessionManager, StartRequest};

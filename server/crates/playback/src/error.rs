use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("session not found")]
    SessionNotFound,
    #[error("global transcode limit reached ({0})")]
    GlobalLimit(usize),
    #[error("per-user transcode limit reached ({0})")]
    UserLimit(usize),
    #[error("segment not available: {0}")]
    SegmentUnavailable(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

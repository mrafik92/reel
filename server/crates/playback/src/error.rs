use crate::redact::redact_text;
use thiserror::Error;

/// A failure whose details must stay on the server: clients only ever see the `ref` id, the
/// (redacted) detail is logged under the same id.
#[derive(Debug, Clone)]
pub struct Diag {
    pub id: String,
    detail: String,
}

impl Diag {
    /// Redacts `detail`, assigns a reference id and logs both.
    pub fn new(detail: impl AsRef<str>) -> Self {
        let id: String = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
        let detail = redact_text(detail.as_ref());
        tracing::warn!(diag = %id, detail = %detail, "playback failure");
        Diag { id, detail }
    }

    /// Redacted detail (server side only; never put this in a response).
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("session not found")]
    SessionNotFound,
    #[error("global transcode limit reached ({0})")]
    GlobalLimit(usize),
    #[error("per-user transcode limit reached ({0})")]
    UserLimit(usize),
    #[error("per-user stream limit reached ({0})")]
    UserStreamLimit(usize),
    #[error("the server is busy; try again shortly")]
    Busy,
    /// A variant of a multi-variant session couldn't start (transcode caps or process budget
    /// full); the player switches to another variant (#111).
    #[error("this quality isn't available right now: {0}")]
    VariantRefused(String),
    #[error("not enough scratch space for another transcode right now")]
    DiskBudget,
    #[error("segment unavailable (ref {})", .0.id)]
    SegmentUnavailable(Diag),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("media processing failed (ref {})", .0.id)]
    Ffmpeg(Diag),
    #[error("i/o error")]
    Io(#[from] std::io::Error),
}

impl PlaybackError {
    pub fn ffmpeg(detail: impl AsRef<str>) -> Self {
        PlaybackError::Ffmpeg(Diag::new(detail))
    }
    pub fn segment_unavailable(detail: impl AsRef<str>) -> Self {
        PlaybackError::SegmentUnavailable(Diag::new(detail))
    }
}

//! Axum routes serving HLS output of transcode/remux sessions.
//!
//! Mount with `app.merge(reel_playback::router::stream_router(manager))` under `/api/v1`.
//! The session id (UUIDv4) acts as the capability; the API lane is expected to add signed-URL
//! checks in front if desired.

use crate::error::PlaybackError;
use crate::session::SessionManager;
use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;

impl IntoResponse for PlaybackError {
    fn into_response(self) -> Response {
        let code = match &self {
            PlaybackError::SessionNotFound => StatusCode::NOT_FOUND,
            PlaybackError::Invalid(_) => StatusCode::BAD_REQUEST,
            PlaybackError::GlobalLimit(_)
            | PlaybackError::UserLimit(_)
            | PlaybackError::UserStreamLimit(_)
            | PlaybackError::VariantRefused(_) => StatusCode::TOO_MANY_REQUESTS,
            PlaybackError::SegmentUnavailable(_)
            | PlaybackError::Busy
            | PlaybackError::DiskBudget => StatusCode::SERVICE_UNAVAILABLE,
            PlaybackError::Ffmpeg(_) | PlaybackError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, self.to_string()).into_response()
    }
}

const M3U8: &str = "application/vnd.apple.mpegurl";

async fn master(
    State(m): State<Arc<SessionManager>>,
    Path(sid): Path<String>,
) -> Result<Response, PlaybackError> {
    let body = m.master_playlist(&sid)?;
    Ok((
        [
            (header::CONTENT_TYPE, M3U8),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response())
}

async fn variant_file(
    State(m): State<Arc<SessionManager>>,
    Path((sid, variant, file)): Path<(String, String, String)>,
) -> Result<Response, PlaybackError> {
    if file == "index.m3u8" {
        let body = m.variant_playlist(&sid, &variant).await?;
        return Ok((
            [
                (header::CONTENT_TYPE, M3U8),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            body,
        )
            .into_response());
    }
    let path = m.variant_segment_path(&sid, &variant, &file).await?;
    let bytes = tokio::fs::read(path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "video/mp4"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response())
}

pub fn stream_router(manager: Arc<SessionManager>) -> Router {
    Router::new()
        .route("/stream/{sid}/master.m3u8", get(master))
        .route("/stream/{sid}/{variant}/{file}", get(variant_file))
        .with_state(manager)
}

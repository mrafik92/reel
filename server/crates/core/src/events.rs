//! Events pushed to clients over WebSocket `/api/v1/events`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request lifecycle state (SPEC §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Requested,
    Approved,
    Searching,
    NotFound,
    Queued,
    Downloading,
    Stalled,
    Importing,
    Available,
    Failed,
    Declined,
}

/// Server-to-client event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Download/request progress; `progress` is 0..1.
    RequestProgress {
        request_id: i64,
        tmdb_id: i64,
        state: RequestState,
        progress: f32,
        eta_seconds: Option<u64>,
        speed_bps: Option<u64>,
    },
    /// Library scan progress.
    ScanProgress {
        library_id: i64,
        scanned: u64,
        total: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scan_progress_shape() {
        let e = ServerEvent::ScanProgress {
            library_id: 1,
            scanned: 2,
            total: 3,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type":"scan_progress","library_id":1,"scanned":2,"total":3})
        );
    }

    #[test]
    fn request_progress_shape() {
        let e = ServerEvent::RequestProgress {
            request_id: 7,
            tmdb_id: 438631,
            state: RequestState::Downloading,
            progress: 0.5,
            eta_seconds: Some(90),
            speed_bps: None,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type":"request_progress","request_id":7,"tmdb_id":438631,
                   "state":"downloading","progress":0.5,"eta_seconds":90,"speed_bps":null})
        );
        assert_eq!(
            serde_json::to_value(RequestState::NotFound).unwrap(),
            json!("not_found")
        );
    }
}

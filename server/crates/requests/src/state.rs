//! Pure mapping from Arr queue items to the request state machine (SPEC §7.1).

use std::time::Duration;

use reel_core::RequestState;

use crate::{
    arr::{QueueItem, parse_timeleft},
    models::ItemProgress,
};

/// Fraction downloaded for a queue item (`1 - sizeleft/size`), clamped to 0..1.
pub fn item_progress(item: &QueueItem) -> f32 {
    if item.size <= 0.0 {
        return 0.0;
    }
    (1.0 - item.sizeleft / item.size).clamp(0.0, 1.0) as f32
}

/// Classify one queue item.
pub fn classify(item: &QueueItem) -> RequestState {
    let status = item.status.as_deref().unwrap_or("").to_ascii_lowercase();
    let state = item
        .tracked_download_state
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let tds = item
        .tracked_download_status
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    if status == "failed" || state == "failed" || state == "failedpending" {
        return RequestState::Failed;
    }
    if state == "importblocked" {
        return RequestState::Stalled;
    }
    if matches!(state.as_str(), "importpending" | "importing" | "imported") || status == "completed"
    {
        return RequestState::Importing;
    }
    if tds == "error" {
        return RequestState::Failed;
    }
    if tds == "warning"
        || matches!(
            status.as_str(),
            "warning" | "paused" | "downloadclientunavailable"
        )
    {
        return RequestState::Stalled;
    }
    if status == "downloading" {
        RequestState::Downloading
    } else {
        RequestState::Queued
    }
}

/// Aggregated view of all queue items belonging to one request.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    pub state: RequestState,
    pub progress: f32,
    pub eta_seconds: Option<u64>,
    pub size: u64,
    pub sizeleft: u64,
    pub release_title: Option<String>,
    pub error: Option<String>,
    pub items: Vec<ItemProgress>,
}

/// Size-weighted aggregation. `None` when there are no items.
///
/// State precedence: any `Downloading` → downloading; else `Queued`; else `Stalled`;
/// else `Importing`; else (all failed) `Failed`.
pub fn aggregate(items: &[&QueueItem]) -> Option<Aggregate> {
    if items.is_empty() {
        return None;
    }
    let states: Vec<RequestState> = items.iter().map(|i| classify(i)).collect();
    let has = |s| states.contains(&s);
    let state = if has(RequestState::Downloading) {
        RequestState::Downloading
    } else if has(RequestState::Queued) {
        RequestState::Queued
    } else if has(RequestState::Stalled) {
        RequestState::Stalled
    } else if has(RequestState::Importing) {
        RequestState::Importing
    } else {
        RequestState::Failed
    };
    let size: f64 = items.iter().map(|i| i.size.max(0.0)).sum();
    let left: f64 = items
        .iter()
        .map(|i| i.sizeleft.clamp(0.0, i.size.max(0.0)))
        .sum();
    let progress = if size > 0.0 {
        (1.0 - left / size).clamp(0.0, 1.0) as f32
    } else if state == RequestState::Importing {
        1.0
    } else {
        0.0
    };
    let eta = items
        .iter()
        .zip(&states)
        .filter(|(_, s)| matches!(s, RequestState::Downloading | RequestState::Queued))
        .filter_map(|(i, _)| i.timeleft.as_deref().and_then(parse_timeleft))
        .max();
    let error = items
        .iter()
        .zip(&states)
        .filter(|(_, s)| matches!(s, RequestState::Failed | RequestState::Stalled))
        .find_map(|(i, _)| i.error_message.clone());
    Some(Aggregate {
        state,
        progress,
        eta_seconds: eta,
        size: size as u64,
        sizeleft: left as u64,
        release_title: items.iter().find_map(|i| i.title.clone()),
        error,
        items: items
            .iter()
            .zip(&states)
            .map(|(i, s)| ItemProgress {
                title: i.title.clone(),
                state: *s,
                progress: item_progress(i),
                size_bytes: i.size.max(0.0) as u64,
                season: i.season(),
                episode: i.episode_number(),
                download_id: i.download_id.clone(),
            })
            .collect(),
    })
}

/// Downgrade `Downloading` to `Stalled` when no bytes moved for `stall_after`.
pub fn apply_stall(
    state: RequestState,
    unchanged_for: Duration,
    stall_after: Duration,
) -> RequestState {
    if state == RequestState::Downloading && unchanged_for >= stall_after {
        RequestState::Stalled
    } else {
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(status: &str, tds: &str, tdst: &str, size: f64, left: f64) -> QueueItem {
        QueueItem {
            id: 1,
            title: Some("Rel".into()),
            size,
            sizeleft: left,
            status: Some(status.into()),
            tracked_download_state: Some(tds.into()),
            tracked_download_status: Some(tdst.into()),
            timeleft: Some("00:10:00".into()),
            ..Default::default()
        }
    }

    #[test]
    fn classification_table() {
        use RequestState::*;
        let cases = [
            ("queued", "downloading", "ok", Queued),
            ("delay", "downloading", "ok", Queued),
            ("downloading", "downloading", "ok", Downloading),
            ("downloading", "downloading", "warning", Stalled),
            ("warning", "downloading", "ok", Stalled),
            ("paused", "downloading", "ok", Stalled),
            ("completed", "importPending", "ok", Importing),
            ("completed", "importing", "ok", Importing),
            ("completed", "importBlocked", "warning", Stalled),
            ("completed", "downloading", "ok", Importing),
            ("failed", "failed", "error", Failed),
            ("downloading", "failedPending", "ok", Failed),
            ("downloading", "downloading", "error", Failed),
        ];
        for (status, state, tds, want) in cases {
            assert_eq!(
                classify(&q(status, state, tds, 100.0, 50.0)),
                want,
                "{status}/{state}/{tds}"
            );
        }
    }

    #[test]
    fn progress_math() {
        let i = q("downloading", "downloading", "ok", 1000.0, 400.0);
        assert!((item_progress(&i) - 0.6).abs() < 1e-6);
        assert_eq!(
            item_progress(&q("queued", "downloading", "ok", 0.0, 0.0)),
            0.0
        );
        let a = aggregate(&[&i]).unwrap();
        assert_eq!(a.state, RequestState::Downloading);
        assert_eq!(a.eta_seconds, Some(600));
        assert_eq!(a.sizeleft, 400);
    }

    #[test]
    fn aggregate_size_weighted_tv() {
        let mut e1 = q("downloading", "downloading", "ok", 1000.0, 0.0); // done
        e1.season_number = Some(1);
        e1.timeleft = Some("00:00:00".into());
        let mut e2 = q("downloading", "downloading", "ok", 3000.0, 3000.0); // 0%
        e2.timeleft = Some("00:30:00".into());
        let mut e3 = q("completed", "importPending", "ok", 1000.0, 0.0);
        e3.error_message = None;
        let a = aggregate(&[&e1, &e2, &e3]).unwrap();
        assert_eq!(a.state, RequestState::Downloading);
        assert!((a.progress - 0.4).abs() < 1e-6); // (5000-3000)/5000
        assert_eq!(a.eta_seconds, Some(1800));
        assert_eq!(a.items.len(), 3);
        assert_eq!(a.items[0].season, Some(1));
    }

    #[test]
    fn aggregate_precedence() {
        let imp = q("completed", "importPending", "ok", 10.0, 0.0);
        let failed = q("failed", "failed", "error", 10.0, 10.0);
        let warn = q("downloading", "downloading", "warning", 10.0, 5.0);
        let queued = q("queued", "downloading", "ok", 10.0, 10.0);
        assert_eq!(
            aggregate(&[&imp, &failed]).unwrap().state,
            RequestState::Importing
        );
        assert_eq!(aggregate(&[&failed]).unwrap().state, RequestState::Failed);
        assert_eq!(
            aggregate(&[&warn, &imp]).unwrap().state,
            RequestState::Stalled
        );
        assert_eq!(
            aggregate(&[&warn, &queued]).unwrap().state,
            RequestState::Queued
        );
        assert!(aggregate(&[]).is_none());
        let mut f = failed.clone();
        f.error_message = Some("boom".into());
        assert_eq!(aggregate(&[&f]).unwrap().error.as_deref(), Some("boom"));
    }

    #[test]
    fn stall_rule() {
        let ten = Duration::from_secs(600);
        assert_eq!(
            apply_stall(RequestState::Downloading, Duration::from_secs(601), ten),
            RequestState::Stalled
        );
        assert_eq!(
            apply_stall(RequestState::Downloading, Duration::from_secs(30), ten),
            RequestState::Downloading
        );
        assert_eq!(
            apply_stall(RequestState::Queued, Duration::from_secs(9999), ten),
            RequestState::Queued
        );
    }
}

//! State shared by the service and the tracker task.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use reel_core::{RequestState, ServerEvent};
use reel_metadata::TmdbClient;
use sqlx::SqlitePool;
use tokio::sync::{Notify, broadcast};

use crate::{
    Error, Result,
    models::{
        ARR_COLS, ArrServer, ArrServerRow, Progress, REQUEST_COLS, RequestRecord, RequestRow,
        state_str,
    },
    notify::{Notification, Notifier, NotifyKind},
};

/// Tracker tuning knobs.
#[derive(Debug, Clone)]
pub struct TrackerConfig {
    /// Queue poll interval while requests are active.
    pub poll_interval: Duration,
    /// No byte progress for this long ⇒ `stalled`.
    pub stall_after: Duration,
    /// Wait this long in `searching` before a finished search command means `not_found`.
    pub search_grace: Duration,
    /// Minimum gap between non-state-change progress events per request.
    pub emit_throttle: Duration,
    /// Idle re-check interval (safety net when no wake-ups arrive).
    pub idle_recheck: Duration,
    pub queue_page_size: u32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            stall_after: Duration::from_secs(600),
            search_grace: Duration::from_secs(45),
            emit_throttle: Duration::from_secs(1),
            idle_recheck: Duration::from_secs(60),
            queue_page_size: 500,
        }
    }
}

pub(crate) struct Shared {
    pub pool: SqlitePool,
    pub tmdb: Option<TmdbClient>,
    pub events: broadcast::Sender<ServerEvent>,
    pub progress: DashMap<i64, Progress>,
    pub last_emit: DashMap<i64, (Instant, Progress)>,
    pub wake: Notify,
    pub notifier: Notifier,
    pub cfg: TrackerConfig,
    pub(crate) downloads: tokio::sync::Mutex<crate::downloads::DlCache>,
}

impl Shared {
    pub async fn get_request(&self, id: i64) -> Result<RequestRecord> {
        sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(Into::into)
        .ok_or_else(|| Error::NotFound(format!("request {id}")))
    }

    pub async fn get_server(&self, id: i64) -> Result<Option<ArrServer>> {
        Ok(sqlx::query_as::<_, ArrServerRow>(&format!(
            "SELECT {ARR_COLS} FROM arr_servers WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(Into::into))
    }

    /// Store the latest progress and emit an event: always on state change, otherwise
    /// throttled per request and de-duplicated.
    pub fn publish(&self, p: Progress, force: bool) {
        self.progress.insert(p.request_id, p.clone());
        let now = Instant::now();
        if let Some(prev) = self.last_emit.get(&p.request_id) {
            let (t, last) = &*prev;
            if *last == p {
                return;
            }
            if !force && last.state == p.state && now.duration_since(*t) < self.cfg.emit_throttle {
                return;
            }
        }
        let _ = self.events.send(p.to_event());
        self.last_emit.insert(p.request_id, (now, p));
    }

    /// Persist a state transition (if it is one) and publish progress.
    /// Returns whether the persisted status changed.
    pub async fn transition(&self, req: &RequestRecord, p: Progress) -> Result<bool> {
        let changed = sqlx::query(
            "UPDATE requests SET status = ?, updated_at = unixepoch() WHERE id = ? AND status != ?",
        )
        .bind(state_str(p.state))
        .bind(req.id)
        .bind(state_str(p.state))
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0;
        let state = p.state;
        self.publish(p, changed);
        if changed {
            match state {
                RequestState::Failed => self.notify(NotifyKind::Failed, req, None),
                RequestState::Available => self.notify(NotifyKind::Available, req, None),
                _ => {}
            }
            if matches!(
                state,
                RequestState::Searching | RequestState::Queued | RequestState::Downloading
            ) {
                self.wake.notify_one();
            }
        }
        Ok(changed)
    }

    /// Persist `request_downloads` rows for the queue items of a progress snapshot.
    pub async fn persist_downloads(&self, req_id: i64, p: &Progress) -> Result<()> {
        for (n, it) in p.items.iter().enumerate() {
            let did = it
                .download_id
                .clone()
                .unwrap_or_else(|| format!("item-{n}"));
            sqlx::query(
                "INSERT INTO request_downloads (request_id, download_id, state, release_title, size, last_progress, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, unixepoch())
                 ON CONFLICT(request_id, download_id) DO UPDATE SET
                   state = excluded.state, release_title = excluded.release_title,
                   size = excluded.size, last_progress = excluded.last_progress,
                   updated_at = excluded.updated_at",
            )
            .bind(req_id)
            .bind(did)
            .bind(state_str(it.state))
            .bind(&it.title)
            .bind(it.size_bytes as i64)
            .bind(it.progress as f64)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Fire-and-forget notification about a request.
    pub fn notify(&self, kind: NotifyKind, req: &RequestRecord, message: Option<String>) {
        let notifier = self.notifier.clone();
        let pool = self.pool.clone();
        let req = req.clone();
        tokio::spawn(async move {
            let user: Option<(String,)> = sqlx::query_as("SELECT username FROM users WHERE id = ?")
                .bind(req.user_id)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
            let n = Notification {
                event: kind,
                request_id: Some(req.id),
                title: req
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("TMDB #{}", req.tmdb_id)),
                media_type: Some(req.media_type.as_str().to_string()),
                tmdb_id: Some(req.tmdb_id),
                user: user.map(|u| u.0),
                message,
            };
            let _ = notifier.send_all(&n).await;
        });
    }

    /// Does the library hold a finished item for this TMDB id?
    pub async fn library_has(&self, media_type: crate::MediaType, tmdb_id: i64) -> Result<bool> {
        let sql = match media_type {
            crate::MediaType::Movie => {
                "SELECT EXISTS(SELECT 1 FROM media_items i WHERE i.kind = 'movie' AND i.tmdb_id = ?1
                   AND EXISTS (SELECT 1 FROM media_files f WHERE f.item_id = i.id))"
            }
            crate::MediaType::Tv => {
                "SELECT EXISTS(SELECT 1 FROM media_items s WHERE s.kind = 'show' AND s.tmdb_id = ?1
                   AND EXISTS (SELECT 1 FROM media_items e WHERE e.kind = 'episode'
                     AND (e.parent_id = s.id
                          OR e.parent_id IN (SELECT id FROM media_items WHERE parent_id = s.id AND kind = 'season'))))"
            }
        };
        let (b,): (i64,) = sqlx::query_as(sql)
            .bind(tmdb_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(b != 0)
    }
}

pub(crate) type SharedRef = Arc<Shared>;

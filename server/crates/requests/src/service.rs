//! The request workflow service: create / approve / decline / retry / list / availability / webhooks.

use std::{collections::HashMap, sync::Arc};

use dashmap::DashMap;
use reel_core::{RequestState, ServerEvent};
use reel_metadata::TmdbClient;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::{
    sync::{Notify, broadcast},
    task::JoinHandle,
};

use crate::{
    Error, MediaType, Result,
    discover::Discovery,
    models::{Progress, REQUEST_COLS, RequestRecord, RequestRow, parse_state, state_str},
    notify::{Notifier, NotifyKind, RetryPolicy},
    shared::{Shared, SharedRef, TrackerConfig},
    tracker::Tracker,
    webhook::{self, ArrEventKind},
};

/// Badge shown on discovery cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Partially,
    Requested,
    None,
}

/// Extra options when creating a request ("⋯" advanced menu).
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// TV only; `None` = all seasons.
    pub seasons: Option<Vec<i64>>,
    pub arr_server_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub root_folder: Option<String>,
}

/// Filter for [`RequestsService::list`].
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub status: Option<RequestState>,
    pub user_id: Option<i64>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Result of processing an Arr webhook.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookOutcome {
    pub event: ArrEventKind,
    pub tmdb_id: Option<i64>,
    /// Requests whose state was touched.
    pub request_ids: Vec<i64>,
    /// Files imported — the API layer should trigger a targeted scanner rescan of these.
    pub imported_paths: Vec<String>,
    /// Files removed by the Arr.
    pub removed_paths: Vec<String>,
}

/// Construction options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub tracker: TrackerConfig,
    pub notify_retry: RetryPolicy,
}

/// `(role, quota_movies, quota_tv, quota_window_days, auto_approve)`.
type UserLimitsRow = (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// Request service handle (cheap to clone).
#[derive(Clone)]
pub struct RequestsService {
    pub(crate) s: SharedRef,
}

impl RequestsService {
    pub fn new(
        pool: SqlitePool,
        tmdb: Option<TmdbClient>,
        events: broadcast::Sender<ServerEvent>,
    ) -> Self {
        Self::with_options(pool, tmdb, events, Options::default())
    }

    pub fn with_options(
        pool: SqlitePool,
        tmdb: Option<TmdbClient>,
        events: broadcast::Sender<ServerEvent>,
        opts: Options,
    ) -> Self {
        let notifier = Notifier::new(pool.clone(), opts.notify_retry);
        RequestsService {
            s: Arc::new(Shared {
                pool,
                tmdb,
                events,
                progress: DashMap::new(),
                last_emit: DashMap::new(),
                wake: Notify::new(),
                notifier,
                cfg: opts.tracker,
            }),
        }
    }

    /// Spawn the background download tracker. Abort the handle to stop it.
    pub fn spawn_tracker(&self) -> JoinHandle<()> {
        let t = Tracker::new(self.s.clone());
        self.s.wake.notify_one(); // pick up requests persisted before startup
        tokio::spawn(t.run())
    }

    /// Wake the tracker (new approvals, webhooks).
    pub fn wake(&self) {
        self.s.wake.notify_one();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.s.events.subscribe()
    }

    pub fn notifier(&self) -> &Notifier {
        &self.s.notifier
    }

    /// TMDB discovery helpers (None-safe: errors if TMDB is not configured).
    pub fn discovery(&self) -> Discovery {
        Discovery::new(self.s.clone())
    }

    // ---- reads -----------------------------------------------------------------------

    pub async fn get(&self, id: i64) -> Result<RequestRecord> {
        self.s.get_request(id).await
    }

    pub async fn list(&self, f: &ListFilter) -> Result<Vec<RequestRecord>> {
        let mut sql = format!("SELECT {REQUEST_COLS} FROM requests WHERE 1=1");
        if f.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        if f.user_id.is_some() {
            sql.push_str(" AND user_id = ?");
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?");
        let mut q = sqlx::query_as::<_, RequestRow>(&sql);
        if let Some(s) = f.status {
            q = q.bind(state_str(s));
        }
        if let Some(u) = f.user_id {
            q = q.bind(u);
        }
        let rows = q
            .bind(f.limit.unwrap_or(100).clamp(1, 1000))
            .bind(f.offset.unwrap_or(0).max(0))
            .fetch_all(&self.s.pool)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Snapshot for `GET /requests/active`: every unfinished request with its latest progress.
    pub async fn active_snapshot(&self) -> Result<Vec<Progress>> {
        let rows: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests WHERE status NOT IN ('available','declined') ORDER BY id"
        ))
        .fetch_all(&self.s.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
        Ok(rows
            .iter()
            .map(|r| match self.s.progress.get(&r.id) {
                Some(p) if p.state == r.status => p.clone(),
                _ => Progress::basic(r, r.status),
            })
            .collect())
    }

    // ---- workflow --------------------------------------------------------------------

    /// Create a request; admins / auto-approve users are approved and pushed to the Arr at once.
    pub async fn create_request(
        &self,
        user_id: i64,
        media_type: MediaType,
        tmdb_id: i64,
        seasons: Option<Vec<i64>>,
    ) -> Result<RequestRecord> {
        self.create_request_with(
            user_id,
            media_type,
            tmdb_id,
            CreateOptions {
                seasons,
                ..Default::default()
            },
        )
        .await
    }

    pub async fn create_request_with(
        &self,
        user_id: i64,
        media_type: MediaType,
        tmdb_id: i64,
        opts: CreateOptions,
    ) -> Result<RequestRecord> {
        let s = &self.s;
        let user: Option<UserLimitsRow> =
            sqlx::query_as(
                "SELECT u.role, l.request_quota_movies, l.request_quota_tv, l.quota_window_days, l.auto_approve
                 FROM users u LEFT JOIN user_limits l ON l.user_id = u.id
                 WHERE u.id = ? AND u.disabled = 0",
            )
            .bind(user_id)
            .fetch_optional(&s.pool)
            .await?;
        let (role, q_movies, q_tv, window, auto) =
            user.ok_or_else(|| Error::NotFound(format!("user {user_id}")))?;
        let is_admin = role == "admin";

        let mut seasons = opts.seasons.clone();
        if media_type == MediaType::Movie {
            seasons = None;
        } else if let Some(v) = seasons.as_mut() {
            v.sort_unstable();
            v.dedup();
            if v.is_empty() {
                seasons = None;
            }
        }

        // Already available / already requested?
        if media_type == MediaType::Movie && s.library_has(media_type, tmdb_id).await? {
            return Err(Error::Conflict("already available in the library".into()));
        }
        let existing: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests WHERE media_type = ? AND tmdb_id = ?
             AND status NOT IN ('declined','failed','not_found')"
        ))
        .bind(media_type.as_str())
        .bind(tmdb_id)
        .fetch_all(&s.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
        let covered = existing.iter().any(|e| match (&e.seasons, &seasons) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(have), Some(want)) => want.iter().all(|w| have.contains(w)),
        });
        if covered {
            return Err(Error::Conflict("already requested".into()));
        }

        // Quota (admins are exempt).
        if !is_admin {
            let quota = match media_type {
                MediaType::Movie => q_movies,
                MediaType::Tv => q_tv,
            };
            if let Some(q) = quota {
                let (used,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM requests WHERE user_id = ? AND media_type = ?
                     AND status != 'declined' AND created_at >= unixepoch() - ? * 86400",
                )
                .bind(user_id)
                .bind(media_type.as_str())
                .bind(window.unwrap_or(7))
                .fetch_one(&s.pool)
                .await?;
                if used >= q {
                    return Err(Error::QuotaExceeded(format!(
                        "request quota reached ({used}/{q} per {} days)",
                        window.unwrap_or(7)
                    )));
                }
            }
        }

        let (title, poster) = self.tmdb_title(media_type, tmdb_id).await;
        let seasons_json = seasons
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO requests (user_id, media_type, tmdb_id, seasons_json, status, arr_id,
                                   quality_profile_id, root_folder, title, poster)
             VALUES (?, ?, ?, ?, 'requested', ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(media_type.as_str())
        .bind(tmdb_id)
        .bind(seasons_json)
        .bind(opts.arr_server_id)
        .bind(opts.quality_profile_id)
        .bind(&opts.root_folder)
        .bind(title)
        .bind(poster)
        .fetch_one(&s.pool)
        .await?;
        let req = s.get_request(id).await?;
        s.publish(Progress::basic(&req, RequestState::Requested), true);
        s.notify(NotifyKind::Requested, &req, None);

        if is_admin || auto.unwrap_or(0) != 0 {
            return self.approve(id, user_id).await;
        }
        Ok(req)
    }

    async fn tmdb_title(
        &self,
        media_type: MediaType,
        tmdb_id: i64,
    ) -> (Option<String>, Option<String>) {
        let Some(t) = &self.s.tmdb else {
            return (None, None);
        };
        match media_type {
            MediaType::Movie => t
                .movie(tmdb_id)
                .await
                .map(|m| (Some(m.title), m.poster_path))
                .unwrap_or((None, None)),
            MediaType::Tv => t
                .tv(tmdb_id)
                .await
                .map(|m| (Some(m.name), m.poster_path))
                .unwrap_or((None, None)),
        }
    }

    /// Approve (or re-push) a request and send it to the Arr. If the Arr call fails the
    /// request stays `approved` with `note` holding the error (use [`Self::retry`]).
    pub async fn approve(&self, request_id: i64, approver_id: i64) -> Result<RequestRecord> {
        let s = &self.s;
        let req = s.get_request(request_id).await?;
        if !matches!(req.status, RequestState::Requested | RequestState::Approved) {
            return Err(Error::InvalidState(format!(
                "cannot approve a request that is {}",
                state_str(req.status)
            )));
        }
        if req.status == RequestState::Requested {
            sqlx::query(
                "UPDATE requests SET status = 'approved', approved_by = ?, updated_at = unixepoch() WHERE id = ?",
            )
            .bind(approver_id)
            .bind(request_id)
            .execute(&s.pool)
            .await?;
            let approved = s.get_request(request_id).await?;
            s.publish(Progress::basic(&approved, RequestState::Approved), true);
            s.notify(NotifyKind::Approved, &approved, None);
        }
        self.push(request_id).await
    }

    /// Push an approved request to the Arr and move it to `searching`/`importing`.
    async fn push(&self, request_id: i64) -> Result<RequestRecord> {
        let s = &self.s;
        let req = s.get_request(request_id).await?;
        match s.dispatch(&req).await {
            Ok(d) => {
                sqlx::query(
                    "UPDATE requests SET arr_kind = ?, arr_id = ?, arr_item_id = ?,
                       quality_profile_id = COALESCE(?, quality_profile_id),
                       root_folder = COALESCE(?, root_folder), note = NULL, updated_at = unixepoch()
                     WHERE id = ?",
                )
                .bind(d.server.kind.as_str())
                .bind(d.server.id)
                .bind(d.item_id)
                .bind(d.profile)
                .bind(&d.root)
                .bind(request_id)
                .execute(&s.pool)
                .await?;
                let req = s.get_request(request_id).await?;
                let mut p = if d.has_file {
                    let mut p = Progress::basic(&req, RequestState::Importing);
                    p.progress = 1.0;
                    p
                } else {
                    Progress::basic(&req, RequestState::Searching)
                };
                p.tmdb_id = req.tmdb_id;
                s.transition(&req, p).await?;
                s.wake.notify_one();
                s.get_request(request_id).await
            }
            Err(e) => {
                tracing::warn!(request = request_id, error = %e, "pushing request to arr failed");
                sqlx::query("UPDATE requests SET note = ?, updated_at = unixepoch() WHERE id = ?")
                    .bind(e.to_string())
                    .bind(request_id)
                    .execute(&s.pool)
                    .await?;
                s.get_request(request_id).await
            }
        }
    }

    pub async fn decline(
        &self,
        request_id: i64,
        decliner_id: i64,
        note: Option<String>,
    ) -> Result<RequestRecord> {
        let s = &self.s;
        let req = s.get_request(request_id).await?;
        if !matches!(req.status, RequestState::Requested | RequestState::Approved) {
            return Err(Error::InvalidState(format!(
                "cannot decline a request that is {}",
                state_str(req.status)
            )));
        }
        sqlx::query(
            "UPDATE requests SET status = 'declined', approved_by = ?, note = ?, updated_at = unixepoch() WHERE id = ?",
        )
        .bind(decliner_id)
        .bind(&note)
        .bind(request_id)
        .execute(&s.pool)
        .await?;
        let req = s.get_request(request_id).await?;
        s.publish(Progress::basic(&req, RequestState::Declined), true);
        s.progress.remove(&request_id);
        s.notify(NotifyKind::Declined, &req, note);
        Ok(req)
    }

    /// Trigger another search for `not_found` / `failed` / `stalled` (or un-pushed approved) requests.
    pub async fn retry(&self, request_id: i64) -> Result<RequestRecord> {
        let s = &self.s;
        let req = s.get_request(request_id).await?;
        match req.status {
            RequestState::Approved => return self.push(request_id).await,
            RequestState::NotFound
            | RequestState::Failed
            | RequestState::Stalled
            | RequestState::Searching => {}
            other => {
                return Err(Error::InvalidState(format!(
                    "cannot retry a request that is {}",
                    state_str(other)
                )));
            }
        }
        let (Some(server_id), Some(item)) = (req.arr_id, req.arr_item_id) else {
            return self.push(request_id).await;
        };
        let server = s
            .get_server(server_id)
            .await?
            .ok_or_else(|| Error::NotConfigured("arr server was removed".into()))?;
        s.search_again(&req, &server, item).await?;
        s.transition(&req, Progress::basic(&req, RequestState::Searching))
            .await?;
        s.wake.notify_one();
        s.get_request(request_id).await
    }

    // ---- availability ----------------------------------------------------------------

    /// Scanner glue: a library item with this TMDB id was ingested. Marks matching in-flight
    /// requests `available` (TV only from `importing`) and notifies the requester.
    /// Returns the ids that flipped.
    pub async fn mark_available_by_tmdb(&self, tmdb_id: i64) -> Result<Vec<i64>> {
        let rows: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests WHERE tmdb_id = ?
             AND status IN ('approved','searching','not_found','queued','downloading','stalled','importing','failed')"
        ))
        .bind(tmdb_id)
        .fetch_all(&self.s.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
        let mut out = vec![];
        for r in rows {
            if r.media_type == MediaType::Tv && r.status != RequestState::Importing {
                continue;
            }
            if self.s.mark_available(&r).await? {
                out.push(r.id);
            }
        }
        Ok(out)
    }

    /// Badge for each TMDB id (movies match `kind='movie'`, tv matches `kind='show'`).
    pub async fn availability_for(
        &self,
        media_type: MediaType,
        tmdb_ids: &[i64],
    ) -> Result<HashMap<i64, Availability>> {
        let ids_json = serde_json::to_string(tmdb_ids).unwrap_or_else(|_| "[]".into());
        let kind = match media_type {
            MediaType::Movie => "movie",
            MediaType::Tv => "show",
        };
        let in_lib: Vec<(i64,)> = sqlx::query_as(
            "SELECT DISTINCT i.tmdb_id FROM media_items i
             WHERE i.kind = ?1 AND i.tmdb_id IN (SELECT value FROM json_each(?2))
               AND (i.kind != 'movie' OR EXISTS (SELECT 1 FROM media_files f WHERE f.item_id = i.id))",
        )
        .bind(kind)
        .bind(&ids_json)
        .fetch_all(&self.s.pool)
        .await?;
        let reqs: Vec<(i64, String)> = sqlx::query_as(
            "SELECT tmdb_id, status FROM requests WHERE media_type = ?1
             AND tmdb_id IN (SELECT value FROM json_each(?2)) AND status != 'declined'",
        )
        .bind(media_type.as_str())
        .bind(&ids_json)
        .fetch_all(&self.s.pool)
        .await?;
        let lib: std::collections::HashSet<i64> = in_lib.into_iter().map(|x| x.0).collect();
        let mut in_flight: std::collections::HashSet<i64> = Default::default();
        for (id, st) in reqs {
            if parse_state(&st) != Some(RequestState::Available) {
                in_flight.insert(id);
            }
        }
        Ok(tmdb_ids
            .iter()
            .map(|id| {
                let a = match (lib.contains(id), in_flight.contains(id), media_type) {
                    (true, true, MediaType::Tv) => Availability::Partially,
                    (true, _, _) => Availability::Available,
                    (false, true, _) => Availability::Requested,
                    (false, false, _) => Availability::None,
                };
                (*id, a)
            })
            .collect())
    }

    // ---- webhooks --------------------------------------------------------------------

    /// Handle a Radarr/Sonarr webhook body. `server_id` (e.g. from `?server=` on the webhook URL)
    /// disambiguates multiple Arr instances. The returned paths let the API trigger targeted rescans.
    pub async fn handle_webhook(
        &self,
        body: &[u8],
        server_id: Option<i64>,
    ) -> Result<WebhookOutcome> {
        let ev = webhook::parse(body)?;
        let s = &self.s;
        let mut outcome = WebhookOutcome {
            event: ev.kind.clone(),
            tmdb_id: ev.tmdb_id,
            request_ids: vec![],
            imported_paths: vec![],
            removed_paths: vec![],
        };
        match ev.kind {
            ArrEventKind::Download => outcome.imported_paths = ev.paths.clone(),
            ArrEventKind::FileDelete => outcome.removed_paths = ev.paths.clone(),
            _ => {}
        }
        let Some(source) = ev.source else {
            return Ok(outcome);
        };
        if matches!(ev.kind, ArrEventKind::Test | ArrEventKind::Other(_)) {
            return Ok(outcome);
        }
        let media_type = match source {
            crate::ArrKind::Radarr => MediaType::Movie,
            crate::ArrKind::Sonarr => MediaType::Tv,
        };
        let reqs: Vec<RequestRecord> = sqlx::query_as::<_, RequestRow>(&format!(
            "SELECT {REQUEST_COLS} FROM requests
             WHERE status NOT IN ('declined','requested')
               AND ((media_type = ?1 AND tmdb_id = ?2) OR (arr_kind = ?3 AND arr_item_id = ?4))
               AND (?5 IS NULL OR arr_id = ?5)"
        ))
        .bind(media_type.as_str())
        .bind(ev.tmdb_id)
        .bind(source.as_str())
        .bind(ev.arr_item_id)
        .bind(server_id)
        .fetch_all(&s.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

        use RequestState::*;
        for req in reqs {
            let disjoint = match &req.seasons {
                Some(sel) => !ev.seasons.is_empty() && !ev.seasons.iter().any(|n| sel.contains(n)),
                None => false,
            };
            if disjoint {
                continue;
            }
            let target: Option<Progress> = match (&ev.kind, req.status) {
                (ArrEventKind::Grab, Approved | Searching | NotFound | Failed) => {
                    let mut p = Progress::basic(&req, Queued);
                    p.release_title = ev.release_title.clone();
                    p.size_bytes = ev.size;
                    p.items = vec![crate::models::ItemProgress {
                        title: ev.release_title.clone(),
                        state: Queued,
                        progress: 0.0,
                        size_bytes: ev.size.unwrap_or(0),
                        season: None,
                        episode: None,
                        download_id: ev.download_id.clone(),
                    }];
                    Some(p)
                }
                (
                    ArrEventKind::Download,
                    Approved | Searching | NotFound | Failed | Queued | Downloading | Stalled,
                ) => {
                    let mut p = Progress::basic(&req, Importing);
                    p.progress = 1.0;
                    p.release_title = ev.release_title.clone();
                    Some(p)
                }
                (
                    ArrEventKind::DownloadFailed,
                    Searching | Queued | Downloading | Stalled | Importing,
                ) => {
                    let mut p = Progress::basic(&req, Failed);
                    p.error = ev.message.clone();
                    Some(p)
                }
                (ArrEventKind::FileDelete, Available)
                    if req.media_type == MediaType::Movie
                        && ev.delete_reason.as_deref() != Some("upgrade") =>
                {
                    Some(Progress::basic(&req, Searching))
                }
                _ => None,
            };
            if let Some(p) = target {
                let items = p.items.clone();
                let changed = s.transition(&req, p.clone()).await?;
                if changed && !items.is_empty() {
                    s.persist_downloads(req.id, &p).await?;
                }
                if changed {
                    outcome.request_ids.push(req.id);
                }
            }
        }
        s.wake.notify_one();
        Ok(outcome)
    }
}

impl Shared {
    /// Flip a request to `available` (persist, emit, notify). Returns whether it changed.
    pub(crate) async fn mark_available(&self, req: &RequestRecord) -> Result<bool> {
        let p = Progress::basic(req, RequestState::Available);
        let changed = self.transition(req, p).await?;
        self.progress.remove(&req.id);
        Ok(changed)
    }
}

//! Background enrichment queue adapting TMDB enrichment to the scanner hook.
//!
//! The scanner calls `enrich` for every created item. Seasons/episodes are
//! mapped to their show, and each movie/show id is queued at most once while
//! pending, so a show is enriched once instead of once per episode. Work runs
//! on a background task and never blocks the scan.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use reel_metadata::TmdbEnricher;
use reel_requests::RequestsService;
use reel_scanner::MetadataProvider;
use sqlx::SqlitePool;
use tokio::sync::mpsc;

pub struct EnrichQueue {
    pool: SqlitePool,
    tx: mpsc::UnboundedSender<i64>,
    pending: Arc<Mutex<HashSet<i64>>>,
}

impl EnrichQueue {
    /// Spawn the worker; must be called inside a tokio runtime.
    /// `requests`: when set, requests are marked available once an enriched item carries their
    /// TMDB id.
    pub fn spawn(
        pool: SqlitePool,
        enricher: Arc<TmdbEnricher>,
        requests: Option<RequestsService>,
    ) -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<i64>();
        let pending: Arc<Mutex<HashSet<i64>>> = Arc::default();
        let (p, pend) = (pool.clone(), pending.clone());
        tokio::spawn(async move {
            while let Some(id) = rx.recv().await {
                match enricher.enrich(&p, id).await {
                    Err(e) => tracing::warn!(item = id, "enrichment failed: {e}"),
                    Ok(()) => {
                        // Scanner glue: a matched library item satisfies in-flight requests.
                        if let Some(rs) = &requests {
                            let tmdb: Option<i64> =
                                sqlx::query_scalar("SELECT tmdb_id FROM media_items WHERE id = ?")
                                    .bind(id)
                                    .fetch_optional(&p)
                                    .await
                                    .ok()
                                    .flatten();
                            if let Some(t) = tmdb {
                                if let Err(e) = rs.mark_available_by_tmdb(t).await {
                                    tracing::warn!(tmdb = t, "mark_available failed: {e}");
                                }
                            }
                        }
                    }
                }
                pend.lock().unwrap().remove(&id);
            }
        });
        Arc::new(Self { pool, tx, pending })
    }

    /// Walk up to the movie/show that owns `item_id`.
    async fn root_of(&self, mut id: i64) -> Result<Option<i64>, sqlx::Error> {
        for _ in 0..4 {
            let row: Option<(String, Option<i64>)> =
                sqlx::query_as("SELECT kind, parent_id FROM media_items WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
            match row {
                None => return Ok(None),
                Some((k, _)) if k == "movie" || k == "show" => return Ok(Some(id)),
                Some((_, Some(p))) => id = p,
                Some(_) => return Ok(None),
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl MetadataProvider for EnrichQueue {
    async fn enrich(&self, item_id: i64) -> reel_core::Result<()> {
        if let Some(root) = self.root_of(item_id).await? {
            let fresh = self.pending.lock().unwrap().insert(root);
            if fresh {
                let _ = self.tx.send(root);
            }
        }
        Ok(())
    }
}

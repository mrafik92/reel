//! Lazy per-channel guide for Xtream providers without an XMLTV feed: `get_short_epg` is called
//! only for channels somebody is looking at (now/next, the guide grid, the tuned channel), cached
//! in `epg_programs`, de-duplicated while in flight and paced by the source's politeness gate.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tracing::{debug, warn};

use crate::{gate::Gates, xtream::XtreamClient};

/// How long a fetched channel guide is considered fresh.
const TTL_SECS: i64 = 30 * 60;
/// Channels fetched per request at most (the rest wait for the next poll).
const MAX_PER_CALL: usize = 24;
const LISTINGS: u32 = 16;

pub struct EpgFetcher {
    pool: SqlitePool,
    http: reqwest::Client,
    gates: Arc<Gates>,
    inflight: Mutex<HashSet<i64>>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type Row = (
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl EpgFetcher {
    pub fn new(pool: SqlitePool, http: reqwest::Client, gates: Arc<Gates>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            http,
            gates,
            inflight: Mutex::default(),
        })
    }

    /// Make sure the guide of these channels is fresh, fetching what is stale in the background.
    /// Waits at most `wait` for the fetch; never fails (EPG must not get in anybody's way).
    pub async fn ensure(self: &Arc<Self>, channel_ids: &[i64], wait: Duration) {
        if channel_ids.is_empty() {
            return;
        }
        let ids = serde_json::to_string(channel_ids).unwrap_or_else(|_| "[]".into());
        let rows: Vec<Row> = match sqlx::query_as(
            "SELECT c.id, c.source_id, c.ext_id, c.tvg_id, c.epg_channel_id, c.epg_manual, \
                    s.url, s.username, s.password, s.user_agent \
             FROM channels c JOIN iptv_sources s ON s.id = c.source_id \
             LEFT JOIN epg_fetch_state f ON f.channel_id = c.id \
             WHERE c.id IN (SELECT value FROM json_each(?)) AND s.kind = 'xtream' \
               AND s.enabled = 1 AND s.epg_mode = 'per_channel' AND c.missing = 0 \
               AND (f.fetched_at IS NULL OR f.fetched_at < ?)",
        )
        .bind(ids)
        .bind(now() - TTL_SECS)
        .fetch_all(&self.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("per-channel guide lookup failed: {e}");
                return;
            }
        };
        // keep the caller's order (visible channels first) and skip what is already being fetched
        let mut todo: Vec<Row> = vec![];
        {
            let mut inflight = self.inflight.lock().unwrap();
            for id in channel_ids {
                if todo.len() >= MAX_PER_CALL {
                    break;
                }
                if let Some(r) = rows.iter().find(|r| r.0 == *id) {
                    if inflight.insert(*id) {
                        todo.push(r.clone());
                    }
                }
            }
        }
        if todo.is_empty() {
            return;
        }
        let me = self.clone();
        let handle = tokio::spawn(async move { me.run(todo).await });
        let _ = tokio::time::timeout(wait, handle).await;
    }

    async fn run(self: Arc<Self>, todo: Vec<Row>) {
        let mut stopped = false;
        let mut touched = std::collections::BTreeSet::new();
        for (id, source_id, ext_id, tvg, epg_id, manual, url, user, pass, ua) in todo {
            if !stopped {
                match self
                    .fetch_one(
                        id,
                        source_id,
                        &ext_id,
                        tvg,
                        epg_id,
                        manual != 0,
                        &url,
                        user,
                        pass,
                        ua,
                    )
                    .await
                {
                    Ok(()) => {
                        touched.insert(source_id);
                    }
                    Err(e) => {
                        // gate closed / provider pushing back: leave the rest for later
                        debug!(channel = id, "per-channel guide paused: {e}");
                        stopped = true;
                    }
                }
            }
            self.inflight.lock().unwrap().remove(&id);
        }
        for s in touched {
            let _ = crate::ingest::update_epg_counts(&self.pool, s).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_one(
        &self,
        channel_id: i64,
        source_id: i64,
        ext_id: &str,
        tvg: Option<String>,
        epg_id: Option<String>,
        manual: bool,
        url: &str,
        user: Option<String>,
        pass: Option<String>,
        ua: Option<String>,
    ) -> crate::Result<()> {
        let (Some(user), Some(pass), Ok(stream_id)) = (user, pass, ext_id.parse::<i64>()) else {
            return Ok(());
        };
        let x = XtreamClient::new(self.http.clone(), url, &user, &pass)
            .with_user_agent(ua)
            .with_gate(self.gates.get(source_id))
            .with_max_wait(Duration::ZERO);
        let listings = x.short_epg(stream_id, LISTINGS).await?;
        // key the programmes by the channel's guide id, or a synthetic one
        let key = epg_id
            .filter(|k| !k.is_empty())
            .or(tvg.filter(|k| !k.is_empty()))
            .unwrap_or_else(|| format!("xc:{source_id}:{stream_id}"));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM epg_programs WHERE channel_tvg_id = ?")
            .bind(&key)
            .execute(&mut *tx)
            .await?;
        if !listings.is_empty() {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO epg_programs (channel_tvg_id, start, stop, title, description) ",
            );
            qb.push_values(&listings, |mut b, l| {
                b.push_bind(&key)
                    .push_bind(l.start)
                    .push_bind(l.stop)
                    .push_bind(&l.title)
                    .push_bind(&l.description);
            });
            qb.build().execute(&mut *tx).await?;
        }
        if !manual {
            sqlx::query(
                "UPDATE channels SET epg_channel_id = ? WHERE id = ? AND epg_channel_id IS NULL",
            )
            .bind(&key)
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO epg_fetch_state (channel_id, fetched_at, programmes) VALUES (?1, ?2, ?3) \
             ON CONFLICT(channel_id) DO UPDATE SET fetched_at = ?2, programmes = ?3",
        )
        .bind(channel_id)
        .bind(now())
        .bind(listings.len() as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

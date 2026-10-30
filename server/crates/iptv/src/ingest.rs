//! Source refresh: playlist/Xtream → channels (diffed, ids stable), XMLTV → EPG (streamed).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use futures_util::StreamExt;
use serde::Serialize;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::{
    Error, Result,
    epg::{self, norm_name},
    m3u::{M3uEntry, M3uParser},
    store::{Source, SourceKind},
    xmltv::{self, Programme, XmlChannel, XmlMsg},
    xtream::XtreamClient,
};

/// How long a vanished channel is kept (hidden) before its row — and favorites — are dropped.
const MISSING_KEEP_SECS: i64 = 30 * 24 * 3600;

pub struct Ingest {
    pub pool: SqlitePool,
    pub http: reqwest::Client,
    pub work_dir: PathBuf,
    /// EPG window: keep programmes ending after `now - back` and starting before `now + fwd`.
    pub epg_back_secs: i64,
    pub epg_fwd_secs: i64,
    /// Allow playlists / guides / provider APIs on private networks (`allow_private_fetch`).
    pub allow_private: bool,
    /// Per-source politeness (rate limit + backoff) shared with every other provider call.
    pub gates: std::sync::Arc<crate::gate::Gates>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ChannelReport {
    pub total: usize,
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct EpgReport {
    pub epg_channels: usize,
    pub programmes_seen: usize,
    pub programmes_kept: usize,
    pub programmes_inserted: usize,
    pub channels_mapped: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RefreshReport {
    pub channels: ChannelReport,
    pub epg: Option<EpgReport>,
    pub epg_error: Option<String>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
struct Incoming {
    ext_id: String,
    name: String,
    number: Option<i64>,
    group: Option<String>,
    logo: Option<String>,
    tvg_id: Option<String>,
    tvg_name: Option<String>,
    stream_url: String,
    catchup_json: Option<String>,
    user_agent: Option<String>,
    referrer: Option<String>,
}

fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(p) = url.strip_prefix("file://") {
        Some(PathBuf::from(p))
    } else if url.starts_with('/') {
        Some(PathBuf::from(url))
    } else {
        None
    }
}

impl Ingest {
    fn request(&self, url: &str, ua: Option<&str>) -> Result<reqwest::RequestBuilder> {
        crate::net::check_url(url, self.allow_private)?;
        Ok(self
            .http
            .get(url)
            .header("User-Agent", ua.unwrap_or(crate::net::DEFAULT_UA)))
    }

    // ───────────── channels ─────────────

    async fn fetch_m3u(&self, src: &Source) -> Result<(Vec<M3uEntry>, Option<String>)> {
        let mut parser = M3uParser::new();
        let mut out = Vec::new();
        if let Some(p) = local_path(&src.url) {
            parser.feed(&tokio::fs::read(p).await?, &mut out);
        } else {
            let resp = self
                .request(&src.url, src.user_agent.as_deref())?
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(Error::Unavailable(format!(
                    "playlist download failed: HTTP {}",
                    resp.status().as_u16()
                )));
            }
            let mut s = resp.bytes_stream();
            while let Some(chunk) = s.next().await {
                parser.feed(&chunk?, &mut out);
            }
        }
        parser.finish(&mut out);
        Ok((out, parser.epg_url))
    }

    fn xtream(&self, src: &Source) -> Result<XtreamClient> {
        let (Some(u), Some(p)) = (&src.username, &src.password) else {
            return Err(Error::Invalid(
                "Xtream sources need a username and password".into(),
            ));
        };
        crate::net::check_url(&src.url, self.allow_private)?;
        Ok(XtreamClient::new(self.http.clone(), &src.url, u, p)
            .with_user_agent(src.user_agent.clone())
            .with_gate(self.gates.get(src.id)))
    }

    async fn gather(&self, src: &Source) -> Result<(Vec<Incoming>, Option<String>, Option<u32>)> {
        match src.kind {
            SourceKind::M3u => {
                let (entries, epg) = self.fetch_m3u(src).await?;
                let mut seen: HashMap<String, usize> = HashMap::new();
                let list = entries
                    .into_iter()
                    .map(|e| {
                        let base = match &e.tvg_id {
                            Some(t) => t.clone(),
                            None => format!(
                                "name:{}|{}",
                                norm_name(&e.name),
                                e.group.as_deref().unwrap_or("").to_lowercase()
                            ),
                        };
                        let n = seen.entry(base.clone()).or_insert(0);
                        let ext_id = if *n == 0 { base } else { format!("{base}#{n}") };
                        *n += 1;
                        Incoming {
                            ext_id,
                            number: e.number,
                            catchup_json: e.catchup.as_ref().map(|c| {
                                serde_json::json!({"kind": c.kind, "days": c.days, "source": c.source})
                                    .to_string()
                            }),
                            name: e.name,
                            group: e.group,
                            logo: e.logo,
                            tvg_id: e.tvg_id,
                            tvg_name: e.tvg_name,
                            stream_url: e.url,
                            user_agent: e.user_agent,
                            referrer: e.referrer,
                        }
                    })
                    .collect();
                Ok((list, epg, None))
            }
            SourceKind::Xtream => {
                let x = self.xtream(src)?;
                let account = x.account_info().await?;
                if !account.active {
                    return Err(Error::Unavailable(format!(
                        "Xtream account is not active ({})",
                        account.status.as_deref().unwrap_or("unknown status")
                    )));
                }
                let cats: HashMap<String, String> =
                    x.live_categories().await?.into_iter().collect();
                let list = x
                    .live_streams()
                    .await?
                    .into_iter()
                    .map(|s| Incoming {
                        ext_id: s.stream_id.to_string(),
                        stream_url: x.stream_url(s.stream_id),
                        group: s.category_id.and_then(|c| cats.get(&c).cloned()),
                        catchup_json: s
                            .archive_days
                            .map(|d| serde_json::json!({"kind": "xc", "days": d}).to_string()),
                        name: s.name,
                        number: s.num,
                        logo: s.icon,
                        tvg_id: s.epg_channel_id,
                        tvg_name: None,
                        user_agent: None,
                        referrer: None,
                    })
                    .collect();
                Ok((list, Some(x.xmltv_url()), account.max_connections))
            }
        }
    }

    /// Fetch the source and diff it into `channels`. Existing rows keep their ids (favorites,
    /// aliases, admin edits survive); vanished channels are hidden, not deleted.
    pub async fn refresh_channels(&self, src: &Source) -> Result<ChannelReport> {
        let (incoming, epg_hint, max_conn) = self.gather(src).await?;
        let now = now_unix();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: HashMap<String, (i64, bool, bool, bool)> =
            sqlx::query_as::<_, (i64, String, i64, i64, i64)>(
                "SELECT id, ext_id, name_custom, number_custom, missing FROM channels WHERE source_id = ?",
            )
            .bind(src.id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|(id, ext, nc, uc, miss)| (ext, (id, nc != 0, uc != 0, miss != 0)))
            .collect();
        let mut report = ChannelReport {
            total: incoming.len(),
            ..Default::default()
        };
        let mut seen: HashSet<i64> = HashSet::with_capacity(incoming.len());
        for c in &incoming {
            match existing.get(&c.ext_id) {
                Some(&(id, name_custom, number_custom, _)) => {
                    seen.insert(id);
                    sqlx::query(
                        "UPDATE channels SET name = CASE WHEN ?2 THEN name ELSE ?3 END, \
                         number = CASE WHEN ?4 THEN number ELSE ?5 END, group_name = ?6, logo = ?7, \
                         tvg_id = ?8, tvg_name = ?9, stream_url = ?10, catchup_json = ?11, \
                         user_agent = ?12, referrer = ?13, missing = 0, missing_since = NULL \
                         WHERE id = ?1",
                    )
                    .bind(id)
                    .bind(name_custom)
                    .bind(&c.name)
                    .bind(number_custom)
                    .bind(c.number)
                    .bind(&c.group)
                    .bind(&c.logo)
                    .bind(&c.tvg_id)
                    .bind(&c.tvg_name)
                    .bind(&c.stream_url)
                    .bind(&c.catchup_json)
                    .bind(&c.user_agent)
                    .bind(&c.referrer)
                    .execute(&mut *tx)
                    .await?;
                    report.updated += 1;
                }
                None => {
                    let id = sqlx::query(
                        "INSERT INTO channels (source_id, ext_id, name, number, group_name, logo, \
                         tvg_id, tvg_name, stream_url, catchup_json, user_agent, referrer) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(src.id)
                    .bind(&c.ext_id)
                    .bind(&c.name)
                    .bind(c.number)
                    .bind(&c.group)
                    .bind(&c.logo)
                    .bind(&c.tvg_id)
                    .bind(&c.tvg_name)
                    .bind(&c.stream_url)
                    .bind(&c.catchup_json)
                    .bind(&c.user_agent)
                    .bind(&c.referrer)
                    .execute(&mut *tx)
                    .await?
                    .last_insert_rowid();
                    seen.insert(id);
                    report.added += 1;
                }
            }
        }
        for (id, _, _, missing) in existing.values() {
            if !seen.contains(id) && !missing {
                sqlx::query("UPDATE channels SET missing = 1, missing_since = ? WHERE id = ?")
                    .bind(now)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                report.missing += 1;
            }
        }
        sqlx::query(
            "DELETE FROM channels WHERE source_id = ? AND missing = 1 AND missing_since < ?",
        )
        .bind(src.id)
        .bind(now - MISSING_KEEP_SECS)
        .execute(&mut *tx)
        .await?;
        // same tvg-id on another source ⇒ failover candidate
        sqlx::query(
            "INSERT OR IGNORE INTO channel_aliases (channel_id, alt_source_channel_id, priority) \
             SELECT a.id, b.id, b.source_id FROM channels a JOIN channels b \
               ON lower(a.tvg_id) = lower(b.tvg_id) AND a.source_id != b.source_id \
             WHERE a.tvg_id != '' AND a.missing = 0 AND b.missing = 0 \
               AND (a.source_id = ?1 OR b.source_id = ?1)",
        )
        .bind(src.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE iptv_sources SET last_refresh_at = ?, last_error = NULL, channel_count = ?, \
             detected_epg_url = ?, max_connections = COALESCE(max_connections, ?) WHERE id = ?",
        )
        .bind(now)
        .bind(report.total as i64)
        .bind(&epg_hint)
        .bind(max_conn.filter(|m| *m > 0).map(i64::from))
        .bind(src.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        info!(source = src.id, ?report, "channels refreshed");
        Ok(report)
    }

    // ───────────── EPG ─────────────

    async fn epg_file(&self, url: &str, ua: Option<&str>, id: i64) -> Result<(PathBuf, bool)> {
        if let Some(p) = local_path(url) {
            return Ok((p, false));
        }
        tokio::fs::create_dir_all(&self.work_dir).await?;
        let dest = self.work_dir.join(format!("epg-{id}.tmp"));
        let gate = self.gates.get(id);
        gate.acquire(std::time::Duration::from_secs(120), true)
            .await?;
        let resp = self.request(url, ua)?.send().await?;
        gate.report(resp.status().as_u16(), crate::gate::retry_after(&resp));
        if !resp.status().is_success() {
            return Err(Error::Unavailable(format!(
                "EPG download failed: HTTP {}",
                resp.status().as_u16()
            )));
        }
        let mut f = tokio::fs::File::create(&dest).await?;
        let mut s = resp.bytes_stream();
        while let Some(chunk) = s.next().await {
            f.write_all(&chunk?).await?;
        }
        f.flush().await?;
        Ok((dest, true))
    }

    async fn flush_epg_channels(&self, chans: &mut Vec<XmlChannel>) -> Result<usize> {
        if chans.is_empty() {
            return Ok(0);
        }
        let n = chans.len();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for c in chans.drain(..) {
            let first = c.names.first().cloned().unwrap_or_else(|| c.id.clone());
            sqlx::query(
                "INSERT INTO epg_channels (id, name, norm_name, icon) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, \
                 norm_name = excluded.norm_name, icon = COALESCE(excluded.icon, icon)",
            )
            .bind(&c.id)
            .bind(&first)
            .bind(norm_name(&first))
            .bind(&c.icon)
            .execute(&mut *tx)
            .await?;
            for name in c.names.iter().chain(std::iter::once(&c.id)) {
                let k = norm_name(name);
                if !k.is_empty() {
                    sqlx::query(
                        "INSERT OR IGNORE INTO epg_channel_names (epg_id, norm_name) VALUES (?, ?)",
                    )
                    .bind(&c.id)
                    .bind(k)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(n)
    }

    async fn insert_programmes(&self, batch: Vec<Programme>) -> Result<usize> {
        let mut inserted = 0;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for chunk in batch.chunks(100) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO epg_programs (channel_tvg_id, start, stop, title, sub_title, \
                 description, category, icon, episode_num) ",
            );
            qb.push_values(chunk, |mut b, p| {
                b.push_bind(&p.channel)
                    .push_bind(p.start)
                    .push_bind(p.stop)
                    .push_bind(&p.title)
                    .push_bind(&p.sub_title)
                    .push_bind(&p.description)
                    .push_bind(&p.category)
                    .push_bind(&p.icon)
                    .push_bind(&p.episode_num);
            });
            qb.build().execute(&mut *tx).await?;
            inserted += chunk.len();
        }
        tx.commit().await?;
        Ok(inserted)
    }

    /// Download + stream-parse the guide, map channels, keep only wanted channels inside the window.
    pub async fn refresh_epg(&self, src: &Source) -> Result<EpgReport> {
        let url = src
            .epg_url
            .clone()
            .or_else(|| src.detected_epg_url.clone())
            .or_else(|| {
                (src.kind == SourceKind::Xtream)
                    .then(|| self.xtream(src).ok().map(|x| x.xmltv_url()))
                    .flatten()
            })
            .ok_or_else(|| Error::Invalid("this source has no EPG URL".into()))?;
        let (path, temp) = self
            .epg_file(&url, src.user_agent.as_deref(), src.id)
            .await?;
        let sniff_path = path.clone();
        let verdict = tokio::task::spawn_blocking(move || xmltv::sniff_guide(&sniff_path))
            .await
            .map_err(|e| Error::Unavailable(format!("guide check crashed: {e}")))?;
        if let Err(e) = verdict {
            if temp {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(e);
        }
        let now = now_unix();
        let window = (now - self.epg_back_secs, now + self.epg_fwd_secs);
        let before_max: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM epg_programs")
            .fetch_one(&self.pool)
            .await?;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<XmlMsg>(4);
        let parse_path = path.clone();
        let parser = tokio::task::spawn_blocking(move || -> Result<xmltv::ParseStats> {
            let reader = xmltv::open_maybe_gz(&parse_path)?;
            xmltv::parse_reader(reader, window, 1000, |m| tx.blocking_send(m).is_ok())
        });

        let mut report = EpgReport::default();
        let mut pending_channels: Vec<XmlChannel> = Vec::new();
        let mut wanted: Option<HashSet<String>> = None;
        while let Some(msg) = rx.recv().await {
            match msg {
                XmlMsg::Channel(c) => {
                    pending_channels.push(c);
                    if pending_channels.len() >= 500 {
                        report.epg_channels +=
                            self.flush_epg_channels(&mut pending_channels).await?;
                    }
                }
                XmlMsg::Programmes(batch) => {
                    if wanted.is_none() {
                        report.epg_channels +=
                            self.flush_epg_channels(&mut pending_channels).await?;
                        report.channels_mapped = epg::map_channels(&self.pool).await?;
                        wanted = Some(
                            sqlx::query_scalar::<_, String>(
                                "SELECT DISTINCT COALESCE(epg_channel_id, tvg_id) FROM channels \
                                 WHERE missing = 0 AND enabled = 1 \
                                 AND COALESCE(epg_channel_id, tvg_id) IS NOT NULL",
                            )
                            .fetch_all(&self.pool)
                            .await?
                            .into_iter()
                            .collect(),
                        );
                    }
                    let w = wanted.as_ref().expect("set above");
                    let keep: Vec<Programme> = batch
                        .into_iter()
                        .filter(|p| w.contains(&p.channel))
                        .collect();
                    report.programmes_inserted += self.insert_programmes(keep).await?;
                }
            }
        }
        report.epg_channels += self.flush_epg_channels(&mut pending_channels).await?;
        let stats = parser
            .await
            .map_err(|e| Error::Unavailable(format!("EPG parser crashed: {e}")))??;
        if temp {
            let _ = tokio::fs::remove_file(&path).await;
        }
        if wanted.is_none() {
            report.channels_mapped = epg::map_channels(&self.pool).await?;
        }
        report.programmes_seen = stats.programmes_seen;
        report.programmes_kept = stats.programmes_kept;
        // Replace: drop the previous rows of every channel this feed delivered, then expire.
        sqlx::query(
            "DELETE FROM epg_programs WHERE id <= ?1 AND channel_tvg_id IN \
             (SELECT DISTINCT channel_tvg_id FROM epg_programs WHERE id > ?1)",
        )
        .bind(before_max)
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM epg_programs WHERE stop < ?")
            .bind(now - self.epg_back_secs)
            .execute(&self.pool)
            .await?;
        self.store_epg_stats(src.id).await?;
        // Never report success without data.
        if report.programmes_seen == 0 {
            return Err(Error::Parse(if report.epg_channels == 0 {
                "the guide contained no channels and no programmes".into()
            } else {
                format!(
                    "the guide returned 0 programmes ({} channels, none with schedule data)",
                    report.epg_channels
                )
            }));
        }
        if report.programmes_inserted == 0 {
            let days = self.epg_fwd_secs / 86_400;
            return Err(Error::Parse(if report.programmes_kept == 0 {
                format!(
                    "the guide has {} programmes but none fall inside the {days}-day window \
                     (check the server clock / guide timestamps)",
                    report.programmes_seen
                )
            } else {
                format!(
                    "the guide has {} programmes but none belong to your channels \
                     ({} of your channels matched a guide channel); \
                     the playlist has no tvg-ids and names did not match",
                    report.programmes_kept, report.channels_mapped
                )
            }));
        }
        sqlx::query(
            "UPDATE iptv_sources SET last_epg_at = ?, epg_mode = 'xmltv', epg_note = NULL, \
             last_error = CASE WHEN last_error LIKE 'EPG:%' THEN NULL ELSE last_error END \
             WHERE id = ?",
        )
        .bind(now)
        .bind(src.id)
        .execute(&self.pool)
        .await?;
        info!(source = src.id, ?report, "EPG refreshed");
        Ok(report)
    }

    /// Persist guide statistics for the admin UI.
    async fn store_epg_stats(&self, source_id: i64) -> Result<()> {
        update_epg_counts(&self.pool, source_id).await
    }

    /// Re-run only the guide import; failures are recorded on the source (`last_error`).
    pub async fn refresh_epg_recorded(&self, src: &Source) -> Result<EpgReport> {
        match self.refresh_epg(src).await {
            Ok(r) => Ok(r),
            Err(e) => {
                self.record_epg_failure(src, &e).await;
                Err(e)
            }
        }
    }

    /// The XMLTV guide is unusable. Xtream sources fall back to the lazy per-channel guide
    /// (not an error: the reason is kept in `epg_note`); other sources record the error.
    async fn record_epg_failure(&self, src: &Source, e: &Error) {
        let msg = crate::net::redact_text(&e.to_string());
        if src.kind == SourceKind::Xtream {
            info!(
                source = src.id,
                "XMLTV guide unusable ({msg}); using per-channel guide"
            );
            let _ = sqlx::query(
                "UPDATE iptv_sources SET epg_mode = 'per_channel', epg_note = ?, \
                 last_error = CASE WHEN last_error LIKE 'EPG:%' THEN NULL ELSE last_error END \
                 WHERE id = ?",
            )
            .bind(&msg)
            .bind(src.id)
            .execute(&self.pool)
            .await;
        } else {
            warn!(source = src.id, "EPG refresh failed: {msg}");
            self.record_error(src.id, &format!("EPG: {msg}")).await;
        }
    }

    /// Channels first, then the guide; failures are recorded on the source.
    pub async fn refresh_source(&self, src: &Source) -> Result<RefreshReport> {
        let channels = match self.refresh_channels(src).await {
            Ok(c) => c,
            Err(e) => {
                self.record_error(src.id, &crate::net::redact_text(&e.to_string()))
                    .await;
                return Err(e);
            }
        };
        let fresh = crate::store::get_source(&self.pool, src.id).await?;
        let (epg, epg_error) = if fresh.epg_url.is_some()
            || fresh.detected_epg_url.is_some()
            || fresh.kind == SourceKind::Xtream
        {
            match self.refresh_epg(&fresh).await {
                Ok(r) => (Some(r), None),
                Err(e) => {
                    self.record_epg_failure(&fresh, &e).await;
                    (None, Some(crate::net::redact_text(&e.to_string())))
                }
            }
        } else {
            (None, None)
        };
        if fresh.kind == SourceKind::Xtream {
            // the VOD catalog never fails a refresh (and waits out provider blocks politely)
            if let Err(e) = self.refresh_vod(&fresh).await {
                warn!(
                    source = src.id,
                    "VOD catalog refresh failed: {}",
                    crate::net::redact_text(&e.to_string())
                );
            }
        }
        Ok(RefreshReport {
            channels,
            epg,
            epg_error,
        })
    }

    async fn record_error(&self, source_id: i64, msg: &str) {
        let _ = sqlx::query("UPDATE iptv_sources SET last_error = ? WHERE id = ?")
            .bind(msg)
            .bind(source_id)
            .execute(&self.pool)
            .await;
    }
}

/// Guide statistics of a source (`epg_*` columns of the admin DTO).
pub async fn update_epg_counts(pool: &SqlitePool, source_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE iptv_sources SET \
           epg_channel_count = (SELECT count(*) FROM epg_channels), \
           epg_programme_count = (SELECT count(*) FROM epg_programs WHERE channel_tvg_id IN \
              (SELECT DISTINCT epg_channel_id FROM channels WHERE source_id = ?1 \
               AND epg_channel_id IS NOT NULL \
               UNION SELECT DISTINCT tvg_id FROM channels WHERE source_id = ?1 \
               AND tvg_id IS NOT NULL)), \
           epg_matched_channels = (SELECT count(*) FROM channels WHERE source_id = ?1 \
              AND missing = 0 AND epg_channel_id IS NOT NULL) \
         WHERE id = ?1",
    )
    .bind(source_id)
    .execute(pool)
    .await?;
    Ok(())
}

//! Database access for sources, channels, favorites and EPG queries, plus the hub's resolver.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Serialize;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    Error, Result,
    hub::{Candidate, ChannelResolver, ChannelSpec},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    M3u,
    Xtream,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Xtream => "xtream",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "m3u" => Some(Self::M3u),
            "xtream" => Some(Self::Xtream),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub kind: SourceKind,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub max_connections: Option<u32>,
    pub epg_url: Option<String>,
    /// Minutes; `None` = default (12 h), `0` = manual only.
    pub refresh_interval: Option<i64>,
    pub user_agent: Option<String>,
    pub enabled: bool,
    pub detected_epg_url: Option<String>,
    pub last_refresh_at: Option<i64>,
    pub last_epg_at: Option<i64>,
    pub last_error: Option<String>,
    pub channel_count: i64,
    /// Guide channels delivered by the last import.
    pub epg_channel_count: i64,
    /// Programmes stored for this source's channels.
    pub epg_programme_count: i64,
    /// This source's channels that are linked to a guide channel.
    pub epg_matched_channels: i64,
    /// `xmltv` | `per_channel` | `none`
    pub epg_mode: String,
    /// Why the XMLTV guide was not usable (when `epg_mode` is `per_channel`).
    pub epg_note: Option<String>,
    pub vod_movie_count: i64,
    pub vod_series_count: i64,
}

pub const SOURCE_COLS: &str = "id, name, kind, url, username, password, max_connections, epg_url, \
    refresh_interval, user_agent, enabled, detected_epg_url, last_refresh_at, last_epg_at, \
    last_error, channel_count, epg_channel_count, epg_programme_count, epg_matched_channels, \
    epg_mode, epg_note, vod_movie_count, vod_series_count";

fn source_from_row(r: &SqliteRow) -> Source {
    Source {
        id: r.get("id"),
        name: r.get("name"),
        kind: SourceKind::parse(&r.get::<String, _>("kind")).unwrap_or(SourceKind::M3u),
        url: r.get("url"),
        username: r.get("username"),
        password: r.get("password"),
        max_connections: r
            .get::<Option<i64>, _>("max_connections")
            .map(|n| n.max(0) as u32),
        epg_url: r.get("epg_url"),
        refresh_interval: r.get("refresh_interval"),
        user_agent: r.get("user_agent"),
        enabled: r.get::<i64, _>("enabled") != 0,
        detected_epg_url: r.get("detected_epg_url"),
        last_refresh_at: r.get("last_refresh_at"),
        last_epg_at: r.get("last_epg_at"),
        last_error: r.get("last_error"),
        channel_count: r.get("channel_count"),
        epg_channel_count: r.get("epg_channel_count"),
        epg_programme_count: r.get("epg_programme_count"),
        epg_matched_channels: r.get("epg_matched_channels"),
        epg_mode: r.get("epg_mode"),
        epg_note: r.get("epg_note"),
        vod_movie_count: r.get("vod_movie_count"),
        vod_series_count: r.get("vod_series_count"),
    }
}

pub async fn list_sources(pool: &SqlitePool) -> Result<Vec<Source>> {
    let rows = sqlx::query(&format!(
        "SELECT {SOURCE_COLS} FROM iptv_sources ORDER BY id"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(source_from_row).collect())
}

pub async fn get_source(pool: &SqlitePool, id: i64) -> Result<Source> {
    sqlx::query(&format!(
        "SELECT {SOURCE_COLS} FROM iptv_sources WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|r| source_from_row(&r))
    .ok_or_else(|| Error::NotFound(format!("source {id}")))
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelRow {
    pub id: i64,
    pub source_id: i64,
    pub name: String,
    pub number: Option<i64>,
    pub group: Option<String>,
    pub logo: Option<String>,
    pub tvg_id: Option<String>,
    pub epg_channel_id: Option<String>,
    pub epg_manual: bool,
    pub enabled: bool,
    pub missing: bool,
    pub has_catchup: bool,
    pub favorite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelFilter {
    pub group: Option<String>,
    pub q: Option<String>,
    pub source_id: Option<i64>,
    /// Compute `favorite` for this user (and filter if `favorites_only`).
    pub user_id: Option<i64>,
    pub favorites_only: bool,
    /// Admin view: include disabled and vanished channels.
    pub include_hidden: bool,
}

fn channel_from_row(r: &SqliteRow) -> ChannelRow {
    ChannelRow {
        id: r.get("id"),
        source_id: r.get("source_id"),
        name: r.get("name"),
        number: r.get("number"),
        group: r.get("group_name"),
        logo: r.get("logo"),
        tvg_id: r.get("tvg_id"),
        epg_channel_id: r.get("epg_channel_id"),
        epg_manual: r.get::<i64, _>("epg_manual") != 0,
        enabled: r.get::<i64, _>("enabled") != 0,
        missing: r.get::<i64, _>("missing") != 0,
        has_catchup: r.get::<Option<String>, _>("catchup_json").is_some(),
        favorite: r.get::<i64, _>("fav") != 0,
    }
}

const CHANNEL_SELECT: &str = "SELECT c.id, c.source_id, c.name, c.number, c.group_name, c.logo, \
    c.tvg_id, c.epg_channel_id, c.epg_manual, c.enabled, c.missing, c.catchup_json, \
    (SELECT count(*) FROM user_channel_favorites f WHERE f.channel_id = c.id AND f.user_id = ?) AS fav \
    FROM channels c JOIN iptv_sources s ON s.id = c.source_id";

pub async fn list_channels(pool: &SqlitePool, f: &ChannelFilter) -> Result<Vec<ChannelRow>> {
    let mut sql = format!("{CHANNEL_SELECT} WHERE 1=1");
    if !f.include_hidden {
        sql.push_str(" AND c.enabled = 1 AND c.missing = 0 AND s.enabled = 1");
    }
    if f.group.is_some() {
        sql.push_str(" AND c.group_name = ?");
    }
    if f.q.is_some() {
        sql.push_str(" AND lower(c.name) LIKE ? ESCAPE '\\'");
    }
    if f.source_id.is_some() {
        sql.push_str(" AND c.source_id = ?");
    }
    if f.favorites_only {
        sql.push_str(" AND fav > 0");
    }
    sql.push_str(" ORDER BY COALESCE(c.number, 1000000), lower(c.name), c.id");
    let mut q = sqlx::query(&sql).bind(f.user_id.unwrap_or(-1));
    if let Some(g) = &f.group {
        q = q.bind(g);
    }
    if let Some(t) = &f.q {
        let esc = t
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        q = q.bind(format!("%{esc}%"));
    }
    if let Some(s) = f.source_id {
        q = q.bind(s);
    }
    Ok(q.fetch_all(pool)
        .await?
        .iter()
        .map(channel_from_row)
        .collect())
}

pub async fn get_channel(pool: &SqlitePool, id: i64, user_id: Option<i64>) -> Result<ChannelRow> {
    sqlx::query(&format!("{CHANNEL_SELECT} WHERE c.id = ?"))
        .bind(user_id.unwrap_or(-1))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(|r| channel_from_row(&r))
        .ok_or_else(|| Error::NotFound(format!("channel {id}")))
}

/// Group names with channel counts (visible channels only).
pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(c.group_name, 'Uncategorized') AS g, count(*) FROM channels c \
         JOIN iptv_sources s ON s.id = c.source_id \
         WHERE c.enabled = 1 AND c.missing = 0 AND s.enabled = 1 GROUP BY g ORDER BY lower(g)",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn set_favorite(
    pool: &SqlitePool,
    user_id: i64,
    channel_id: i64,
    on: bool,
) -> Result<()> {
    get_channel(pool, channel_id, None).await?;
    if on {
        sqlx::query(
            "INSERT INTO user_channel_favorites (user_id, channel_id, rank) VALUES (?, ?, \
             COALESCE((SELECT max(rank) + 1 FROM user_channel_favorites WHERE user_id = ?), 0)) \
             ON CONFLICT(user_id, channel_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(channel_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM user_channel_favorites WHERE user_id = ? AND channel_id = ?")
            .bind(user_id)
            .bind(channel_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// The channels most users favorited (warm-channel candidates).
pub async fn top_favorites(pool: &SqlitePool, n: usize) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT f.channel_id FROM user_channel_favorites f \
         JOIN channels c ON c.id = f.channel_id JOIN iptv_sources s ON s.id = c.source_id \
         WHERE c.enabled = 1 AND c.missing = 0 AND s.enabled = 1 \
         GROUP BY f.channel_id ORDER BY count(*) DESC, f.channel_id LIMIT ?",
    )
    .bind(n as i64)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Default)]
pub struct ChannelPatch {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    /// `Some(None)` clears the custom number.
    pub number: Option<Option<i64>>,
    pub group: Option<Option<String>>,
    /// `Some(Some(id))` = manual EPG mapping, `Some(None)` = back to automatic.
    pub epg_channel_id: Option<Option<String>>,
}

pub async fn update_channel(pool: &SqlitePool, id: i64, p: &ChannelPatch) -> Result<()> {
    let cur = get_channel(pool, id, None).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(e) = p.enabled {
        sqlx::query("UPDATE channels SET enabled = ? WHERE id = ?")
            .bind(i64::from(e))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(n) = &p.name {
        let n = n.trim();
        if n.is_empty() {
            return Err(Error::Invalid("name must not be empty".into()));
        }
        sqlx::query("UPDATE channels SET name = ?, name_custom = 1 WHERE id = ?")
            .bind(n)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(n) = p.number {
        sqlx::query("UPDATE channels SET number = ?, number_custom = ? WHERE id = ?")
            .bind(n)
            .bind(i64::from(n.is_some()))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(g) = &p.group {
        sqlx::query("UPDATE channels SET group_name = ? WHERE id = ?")
            .bind(g)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(m) = &p.epg_channel_id {
        match m {
            Some(epg) => {
                sqlx::query("UPDATE channels SET epg_channel_id = ?, epg_manual = 1 WHERE id = ?")
                    .bind(epg.trim())
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                sqlx::query(
                    "UPDATE channels SET epg_channel_id = NULL, epg_manual = 0 WHERE id = ?",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    let _ = cur;
    if matches!(p.epg_channel_id, Some(None)) {
        crate::epg::map_channels(pool).await?;
    }
    Ok(())
}

pub async fn get_aliases(pool: &SqlitePool, channel_id: i64) -> Result<Vec<(i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT alt_source_channel_id, priority FROM channel_aliases WHERE channel_id = ? \
         ORDER BY priority, alt_source_channel_id",
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?)
}

pub async fn set_aliases(pool: &SqlitePool, channel_id: i64, aliases: &[(i64, i64)]) -> Result<()> {
    get_channel(pool, channel_id, None).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM channel_aliases WHERE channel_id = ?")
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;
    for (alt, prio) in aliases {
        if *alt == channel_id {
            return Err(Error::Invalid("a channel cannot alias itself".into()));
        }
        let r = sqlx::query(
            "INSERT OR REPLACE INTO channel_aliases (channel_id, alt_source_channel_id, priority) \
             VALUES (?, ?, ?)",
        )
        .bind(channel_id)
        .bind(alt)
        .bind(prio)
        .execute(&mut *tx)
        .await;
        if matches!(&r, Err(sqlx::Error::Database(d)) if d.is_foreign_key_violation()) {
            return Err(Error::Invalid(format!("channel {alt} does not exist")));
        }
        r?;
    }
    tx.commit().await?;
    Ok(())
}

// ───────────────────────── EPG queries ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EpgProgram {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub sub_title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    pub episode_num: Option<String>,
}

/// Programmes overlapping `[from, to)` per channel id (only the requested channels).
pub async fn epg_range(
    pool: &SqlitePool,
    channel_ids: &[i64],
    from: i64,
    to: i64,
    detail: bool,
) -> Result<HashMap<i64, Vec<EpgProgram>>> {
    let ids = serde_json::to_string(channel_ids).unwrap_or_else(|_| "[]".into());
    let rows = sqlx::query(
        "SELECT c.id AS cid, p.start, p.stop, p.title, p.sub_title, p.description, p.category, \
         p.icon, p.episode_num FROM channels c \
         JOIN epg_programs p ON p.channel_tvg_id = COALESCE(c.epg_channel_id, c.tvg_id) \
         WHERE c.id IN (SELECT value FROM json_each(?)) AND p.stop > ? AND p.start < ? \
         ORDER BY c.id, p.start",
    )
    .bind(ids)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<i64, Vec<EpgProgram>> = HashMap::new();
    for r in rows {
        out.entry(r.get("cid")).or_default().push(EpgProgram {
            start: r.get("start"),
            stop: r.get("stop"),
            title: r.get("title"),
            sub_title: r.get("sub_title"),
            description: if detail { r.get("description") } else { None },
            category: r.get("category"),
            icon: if detail { r.get("icon") } else { None },
            episode_num: if detail { r.get("episode_num") } else { None },
        });
    }
    Ok(out)
}

/// `(now, next)` programme per channel at time `at`.
pub async fn now_next(
    pool: &SqlitePool,
    channel_ids: &[i64],
    at: i64,
) -> Result<HashMap<i64, (Option<EpgProgram>, Option<EpgProgram>)>> {
    let ranges = epg_range(pool, channel_ids, at, at + 12 * 3600, true).await?;
    let mut out = HashMap::new();
    for (cid, progs) in ranges {
        let now = progs.iter().find(|p| p.start <= at && p.stop > at).cloned();
        let after = now.as_ref().map_or(at, |n| n.stop);
        let next = progs.iter().find(|p| p.start >= after).cloned();
        out.insert(cid, (now, next));
    }
    Ok(out)
}

// ───────────────────────── resolver ─────────────────────────

/// Resolves channels (with alias failover candidates) straight from the database.
pub struct DbResolver {
    pub pool: SqlitePool,
}

const CAND_SELECT: &str = "SELECT c.id, c.name, c.stream_url, c.user_agent AS c_ua, c.referrer, \
    s.id AS source_id, s.user_agent AS s_ua, s.max_connections \
    FROM channels c JOIN iptv_sources s ON s.id = c.source_id \
    WHERE c.enabled = 1 AND c.missing = 0 AND s.enabled = 1";

fn candidate_from_row(r: &SqliteRow) -> Candidate {
    Candidate {
        channel_id: r.get("id"),
        source_id: r.get("source_id"),
        url: r.get("stream_url"),
        user_agent: r
            .get::<Option<String>, _>("c_ua")
            .or_else(|| r.get::<Option<String>, _>("s_ua")),
        referrer: r.get("referrer"),
        max_connections: r
            .get::<Option<i64>, _>("max_connections")
            .map(|n| n.max(0) as u32),
    }
}

#[async_trait]
impl ChannelResolver for DbResolver {
    async fn resolve(&self, channel_id: i64) -> Result<Option<ChannelSpec>> {
        let Some(row) = sqlx::query(&format!("{CAND_SELECT} AND c.id = ?"))
            .bind(channel_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let mut candidates = vec![candidate_from_row(&row)];
        let alts = sqlx::query(&format!(
            "{CAND_SELECT} AND c.id IN (SELECT alt_source_channel_id FROM channel_aliases \
             WHERE channel_id = ?) ORDER BY (SELECT priority FROM channel_aliases \
             WHERE channel_id = ? AND alt_source_channel_id = c.id), c.id"
        ))
        .bind(channel_id)
        .bind(channel_id)
        .fetch_all(&self.pool)
        .await?;
        candidates.extend(alts.iter().map(candidate_from_row));
        Ok(Some(ChannelSpec {
            channel_id,
            name: row.get("name"),
            candidates,
        }))
    }
}

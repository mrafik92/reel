//! Several logins on one Xtream provider (#297).
//!
//! Providers sell extra connections as extra logins on the same server. Each login is a source
//! of its own (credentials, connection limit), and each one imports the provider's catalog, so
//! every channel and title would show once per login. Logins on the same server (scheme, host and
//! port) form a *provider group*, ordered enabled first, then by id (the oldest enabled login is
//! the main one):
//!
//! - a row a later login shares with an earlier one (same provider id) is flagged `duplicate`
//!   and left out of listings; rows only one login has stay visible;
//! - playing a visible row can use any enabled login of the group ([`mirrors`]), so each extra
//!   login is an extra connection;
//! - watch progress and channel favourites saved on a row that became a duplicate move to the
//!   visible row.
//!
//! Nothing here calls the provider: it works on what the catalog refreshes already stored.

use std::collections::BTreeMap;

use sqlx::SqlitePool;
use tracing::info;

use crate::{
    Result,
    store::{self, Source, SourceKind},
};

/// The provider server of an Xtream source (`scheme://host:port`); `None` for M3U sources and
/// unparsable URLs.
pub fn provider_key(src: &Source) -> Option<String> {
    if src.kind != SourceKind::Xtream {
        return None;
    }
    let u = reqwest::Url::parse(src.url.trim()).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    Some(format!(
        "{}://{}:{}",
        u.scheme(),
        host,
        u.port_or_known_default()?
    ))
}

/// Provider groups with more than one login, each in precedence order (enabled first, then id).
pub fn groups(sources: &[Source]) -> Vec<Vec<&Source>> {
    let mut by_key: BTreeMap<String, Vec<&Source>> = BTreeMap::new();
    for s in sources {
        if let Some(k) = provider_key(s) {
            by_key.entry(k).or_default().push(s);
        }
    }
    by_key
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|mut g| {
            g.sort_by_key(|s| (!s.enabled, s.id));
            g
        })
        .collect()
}

/// The other enabled logins on `source_id`'s provider, oldest first (empty when it has none).
pub async fn mirrors(pool: &SqlitePool, source_id: i64) -> Result<Vec<Source>> {
    let all = store::list_sources(pool).await?;
    Ok(mirrors_in(&all, source_id).into_iter().cloned().collect())
}

fn mirrors_in(all: &[Source], source_id: i64) -> Vec<&Source> {
    let Some(key) = all
        .iter()
        .find(|s| s.id == source_id)
        .and_then(provider_key)
    else {
        return vec![];
    };
    all.iter()
        .filter(|s| s.id != source_id && s.enabled)
        .filter(|s| provider_key(s).as_deref() == Some(key.as_str()))
        .collect()
}

/// What [`refresh`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DedupReport {
    pub channels: u64,
    pub movies: u64,
    pub series: u64,
    /// Watch-progress and favourite rows moved from a duplicate to the visible row.
    pub moved: u64,
}

/// The tables with a `duplicate` flag and the provider id that identifies a row across logins.
const TABLES: [(&str, &str); 3] = [
    ("channels", "ext_id"),
    ("vod_movies", "stream_id"),
    ("vod_series", "series_id"),
];

/// Recompute the `duplicate` flags of every provider group and move per-user state off rows
/// that became duplicates. Cheap enough to run after every refresh and source change.
pub async fn refresh(pool: &SqlitePool) -> Result<DedupReport> {
    let sources = store::list_sources(pool).await?;
    let groups = groups(&sources);
    let mut rep = DedupReport::default();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    for (table, _) in TABLES {
        sqlx::query(&format!(
            "UPDATE {table} SET duplicate = 0 WHERE duplicate = 1"
        ))
        .execute(&mut *tx)
        .await?;
    }
    for g in &groups {
        let ids: Vec<i64> = g.iter().map(|s| s.id).collect();
        for i in 1..ids.len() {
            let earlier = serde_json::to_string(&ids[..i]).unwrap_or_default();
            for (n, (table, key)) in TABLES.iter().enumerate() {
                let flagged = sqlx::query(&format!(
                    "UPDATE {table} SET duplicate = 1 WHERE source_id = ?1 AND {key} IS NOT NULL \
                     AND EXISTS (SELECT 1 FROM {table} o \
                       WHERE o.source_id IN (SELECT value FROM json_each(?2)) \
                       AND o.{key} = {table}.{key} AND o.missing = 0)"
                ))
                .bind(ids[i])
                .bind(&earlier)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                match n {
                    0 => rep.channels += flagged,
                    1 => rep.movies += flagged,
                    _ => rep.series += flagged,
                }
            }
        }
        let group = serde_json::to_string(&ids).unwrap_or_default();
        rep.moved += move_user_state(&mut tx, &group).await?;
    }
    tx.commit().await?;
    if rep != DedupReport::default() {
        info!(
            ?rep,
            groups = groups.len(),
            "IPTV logins on the same provider merged"
        );
    }
    Ok(rep)
}

/// Keep the newer progress when both rows have some.
const WATCH_UPSERT: &str = "ON CONFLICT (user_id, kind, item_id) DO UPDATE SET \
    position_s = excluded.position_s, duration_s = excluded.duration_s, \
    played = excluded.played, last_played_at = excluded.last_played_at \
    WHERE COALESCE(excluded.last_played_at, 0) > COALESCE(vod_watch.last_played_at, 0)";

/// The visible row of a duplicate is the one of the same group with the same provider id that
/// is neither a duplicate nor missing (there is exactly one).
async fn move_user_state(tx: &mut sqlx::SqliteConnection, group: &str) -> Result<u64> {
    const IN_GROUP: &str = "IN (SELECT value FROM json_each(?1))";
    let mut moved = 0;
    // movies
    sqlx::query(&format!(
        "INSERT INTO vod_watch (user_id, kind, item_id, position_s, duration_s, played, last_played_at) \
         SELECT w.user_id, 'movie', v.id, w.position_s, w.duration_s, w.played, w.last_played_at \
         FROM vod_watch w JOIN vod_movies d ON d.id = w.item_id \
         JOIN vod_movies v ON v.stream_id = d.stream_id AND v.duplicate = 0 AND v.missing = 0 \
         WHERE w.kind = 'movie' AND d.duplicate = 1 AND d.source_id {IN_GROUP} \
           AND v.source_id {IN_GROUP} {WATCH_UPSERT}"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?;
    moved += sqlx::query(&format!(
        "DELETE FROM vod_watch WHERE kind = 'movie' AND item_id IN \
         (SELECT id FROM vod_movies WHERE duplicate = 1 AND source_id {IN_GROUP})"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    // episodes: only where the visible series already has that episode (episode lists are
    // fetched lazily; the rest moves on a later run)
    let episode_map = format!(
        "JOIN vod_series ds ON ds.id = de.series_pk \
         JOIN vod_series vs ON vs.series_id = ds.series_id AND vs.duplicate = 0 AND vs.missing = 0 \
         JOIN vod_episodes ve ON ve.series_pk = vs.id AND ve.ext_id = de.ext_id \
         WHERE ds.duplicate = 1 AND ds.source_id {IN_GROUP} AND vs.source_id {IN_GROUP}"
    );
    sqlx::query(&format!(
        "INSERT INTO vod_watch (user_id, kind, item_id, position_s, duration_s, played, last_played_at) \
         SELECT w.user_id, 'episode', ve.id, w.position_s, w.duration_s, w.played, w.last_played_at \
         FROM vod_watch w JOIN vod_episodes de ON de.id = w.item_id AND w.kind = 'episode' \
         {episode_map} {WATCH_UPSERT}"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?;
    moved += sqlx::query(&format!(
        "DELETE FROM vod_watch WHERE kind = 'episode' AND item_id IN \
         (SELECT de.id FROM vod_episodes de {episode_map})"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    // channel favourites
    sqlx::query(&format!(
        "INSERT OR IGNORE INTO user_channel_favorites (user_id, channel_id, rank, added_at) \
         SELECT f.user_id, v.id, f.rank, f.added_at FROM user_channel_favorites f \
         JOIN channels d ON d.id = f.channel_id \
         JOIN channels v ON v.ext_id = d.ext_id AND v.duplicate = 0 AND v.missing = 0 \
         WHERE d.duplicate = 1 AND d.source_id {IN_GROUP} AND v.source_id {IN_GROUP}"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?;
    moved += sqlx::query(&format!(
        "DELETE FROM user_channel_favorites WHERE channel_id IN \
         (SELECT id FROM channels WHERE duplicate = 1 AND source_id {IN_GROUP})"
    ))
    .bind(group)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: i64, kind: SourceKind, url: &str, enabled: bool) -> Source {
        Source {
            id,
            name: format!("s{id}"),
            kind,
            url: url.into(),
            username: None,
            password: None,
            max_connections: Some(1),
            epg_url: None,
            epg_urls: vec![],
            refresh_interval: None,
            user_agent: None,
            enabled,
            detected_epg_url: None,
            last_refresh_at: None,
            last_epg_at: None,
            last_error: None,
            channel_count: 0,
            epg_channel_count: 0,
            epg_programme_count: 0,
            epg_matched_channels: 0,
            epg_mode: "none".into(),
            epg_note: None,
            vod_movie_count: 0,
            vod_series_count: 0,
        }
    }

    #[test]
    fn provider_key_is_the_server() {
        let x = |u: &str| provider_key(&src(1, SourceKind::Xtream, u, true));
        assert_eq!(
            x("http://Prov.example:8080"),
            x("http://prov.example:8080/")
        );
        assert_eq!(x("http://prov.example"), x("http://prov.example:80"));
        assert_ne!(x("http://prov.example:8080"), x("http://prov.example:8081"));
        assert_ne!(x("http://prov.example"), x("https://prov.example"));
        assert_eq!(x("not a url"), None);
        assert_eq!(
            provider_key(&src(1, SourceKind::M3u, "http://prov.example/a.m3u", true)),
            None
        );
    }

    #[test]
    fn groups_put_enabled_logins_first() {
        let all = vec![
            src(1, SourceKind::Xtream, "http://a.example:8080", false),
            src(2, SourceKind::Xtream, "http://a.example:8080", true),
            src(3, SourceKind::Xtream, "http://b.example", true),
            src(4, SourceKind::Xtream, "http://a.example:8080", true),
            src(5, SourceKind::M3u, "http://a.example:8080/list.m3u", true),
        ];
        let g = groups(&all);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].iter().map(|s| s.id).collect::<Vec<_>>(), [2, 4, 1]);
        let m = |id| {
            mirrors_in(&all, id)
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(m(2), [4]);
        assert_eq!(m(1), [2, 4]);
        assert!(m(3).is_empty());
        assert!(m(5).is_empty());
    }
}

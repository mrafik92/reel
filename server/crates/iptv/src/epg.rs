//! EPG helpers: name normalisation and channel mapping.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::Result;

/// Lower-case alphanumeric key of a channel name: strips quality tags, country prefixes and
/// punctuation so `"UK: BBC One HD"`, `"BBC One"` and `"bbc-one fhd"` collide.
pub fn norm_name(name: &str) -> String {
    let mut s = name.to_lowercase();
    // drop (...) and [...] groups
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.drain(..) {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let mut tokens: Vec<String> = out
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    const NOISE: &[&str] = &[
        "hd", "fhd", "uhd", "sd", "4k", "8k", "hevc", "h265", "h264", "hq", "lq", "raw", "50fps",
        "60fps", "channel", "tv",
    ];
    // leading country/provider tag: "UK: …", "|UK| …", "[UK] …", "UK - …"
    let t = name.trim_start_matches(['|', '[', ' ']);
    let letters: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    if (2..=3).contains(&letters.len()) && tokens.len() > 1 {
        let after = &t[letters.len()..];
        if after.trim_start().starts_with([':', '|', ']']) || after.starts_with(" - ") {
            tokens.remove(0);
        }
    }
    tokens.retain(|t| !NOISE.contains(&t.as_str()));
    tokens.join("")
}

/// Order-insensitive variant (`"One BBC"` ≈ `"BBC One"`).
pub fn token_key(name: &str) -> String {
    let mut parts: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    parts.sort();
    parts.join("")
}

/// Recompute `channels.epg_channel_id` for channels without a manual override:
/// exact `tvg-id` match first, then normalised-name match. Returns the number of channels
/// that now have a mapping.
pub async fn map_channels(pool: &SqlitePool) -> Result<usize> {
    let by_id: HashMap<String, String> =
        sqlx::query_as::<_, (String,)>("SELECT id FROM epg_channels")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id,)| (id.to_lowercase(), id))
            .collect();
    if by_id.is_empty() {
        return Ok(0);
    }
    let mut by_name: HashMap<String, String> = HashMap::new();
    for (n, id) in sqlx::query_as::<_, (String, String)>(
        "SELECT norm_name, epg_id FROM epg_channel_names ORDER BY rowid",
    )
    .fetch_all(pool)
    .await?
    {
        by_name.entry(n).or_insert(id);
    }
    type Row = (i64, String, Option<String>, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, tvg_id, tvg_name, epg_channel_id FROM channels \
         WHERE epg_manual = 0 AND missing = 0",
    )
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    let mut mapped = 0;
    for (id, name, tvg_id, tvg_name, current) in rows {
        let found = tvg_id
            .as_deref()
            .and_then(|t| by_id.get(&t.to_lowercase()))
            .or_else(|| by_name.get(&norm_name(&name)))
            .or_else(|| tvg_name.as_deref().and_then(|n| by_name.get(&norm_name(n))))
            .cloned();
        if found.is_some() {
            mapped += 1;
        }
        if found != current {
            sqlx::query("UPDATE channels SET epg_channel_id = ? WHERE id = ?")
                .bind(&found)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_collides_variants() {
        let k = norm_name("BBC One");
        for v in [
            "BBC One HD",
            "bbc-one fhd",
            "UK: BBC One",
            "|UK| BBC One FHD",
            "BBC One (London)",
            "BBC One [4K]",
        ] {
            assert_eq!(norm_name(v), k, "{v}");
        }
        assert_ne!(norm_name("BBC Two"), k);
        assert_eq!(norm_name("CNN"), "cnn");
        // short real words are not mistaken for country tags without a separator
        assert_eq!(norm_name("ESPN News"), "espnnews");
        assert_eq!(token_key("One BBC"), token_key("BBC One"));
    }
}

//! EPG helpers: name normalisation and channel mapping.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::Result;

/// Unicode-fold for matching: NFKD (so `ᴿᴬᵂ`/`ᴴᴰ`/full-width forms become plain letters), combining
/// marks dropped (accents, Arabic harakat), Arabic letter variants unified, Arabic-Indic digits
/// turned into ASCII digits, lower-case.
fn fold(name: &str) -> String {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    let mut out = String::with_capacity(name.len());
    for c in name.nfkd().filter(|c| !is_combining_mark(*c)) {
        match c {
            '\u{0640}' => {} // tatweel
            '\u{0623}' | '\u{0625}' | '\u{0622}' | '\u{0671}' => out.push('\u{0627}'),
            '\u{0649}' => out.push('\u{064A}'),
            '\u{0629}' => out.push('\u{0647}'),
            '\u{0660}'..='\u{0669}' => out.push((b'0' + (c as u32 - 0x0660) as u8) as char),
            '\u{06F0}'..='\u{06F9}' => out.push((b'0' + (c as u32 - 0x06F0) as u8) as char),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

const NOISE: &[&str] = &[
    "hd", "fhd", "uhd", "sd", "4k", "8k", "hevc", "h265", "h264", "x265", "x264", "hq", "lq",
    "raw", "50fps", "60fps", "hdr", "hdr10", "channel", "tv", "vip", "backup",
];

/// Lower-case alphanumeric key of a channel name: strips quality tags (FHD/HD/4K/HEVC/RAW incl.
/// superscript forms), country prefixes (`AR:`, `UK|`) and punctuation so `"UK: BBC One HD"`,
/// `"BBC One"` and `"bbc-one fhd"` collide. A `+1`-style timeshift suffix is kept (`plus1`) so
/// `"Channel 4 +1"` never collides with `"Channel 4"` or `"Channel 41"`. Arabic and Latin names
/// are both supported (letters unified, digits ASCII).
fn tokens_of(name: &str) -> Vec<String> {
    let s = fold(name).replace("h.265", " ").replace("h.264", " ");
    // `+1` / `+24` timeshift → a word of its own; drop (...) and [...] groups
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '+' if depth == 0 && chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                out.push_str(" plus")
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let mut tokens: Vec<String> = out
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    // trailing single-letter provider tag after quality tags: "MBC 2 FHD HEVC M"
    if tokens.len() >= 3
        && tokens.last().is_some_and(|t| t == "m")
        && tokens
            .get(tokens.len() - 2)
            .is_some_and(|t| NOISE.contains(&t.as_str()))
    {
        tokens.pop();
    }
    // leading country/provider tag: "UK: …", "|UK| …", "[UK] …", "UK - …"
    let t = s.trim_start_matches(['|', '[', ' ']);
    let letters: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    if (2..=3).contains(&letters.len()) && tokens.len() > 1 {
        let after = &t[letters.len()..];
        if after.trim_start().starts_with([':', '|', ']']) || after.starts_with(" - ") {
            tokens.remove(0);
        }
    }
    // leading zeros of numbers ("XTRA 04" = "XTRA 4"), common spelling variants
    for t in tokens.iter_mut() {
        if t.len() > 1 && t.chars().all(|c| c.is_ascii_digit()) && t.starts_with('0') {
            let n = t.trim_start_matches('0');
            *t = if n.is_empty() { "0".into() } else { n.into() };
        }
        if t == "mubasher" {
            *t = "mobasher".into();
        }
    }
    tokens
}

pub fn norm_name(name: &str) -> String {
    let mut t = tokens_of(name);
    t.retain(|t| !NOISE.contains(&t.as_str()));
    t.join("")
}

/// Key of an XMLTV channel *id*. epgshare-style ids are dotted names with the country as the last
/// label (`MBC.2.ae`, `beIN.Sports.1.HD.qa`, `Sky.Sports.Main.Event.uk`): the TLD is dropped and the
/// rest normalised like a name. Other ids are normalised as names.
pub fn norm_id(id: &str) -> String {
    norm_name(&strip_tld(id))
}

/// Country / feed suffix of a dotted id: `.ae`, `.uk`, `.net`, `.com`, `.bein`.
fn strip_tld(id: &str) -> String {
    let t = id.trim();
    if t.contains(char::is_whitespace) || !t.contains('.') {
        return t.to_string();
    }
    let mut parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1].to_lowercase();
        let alpha = last.chars().all(|c| c.is_ascii_alphabetic());
        if alpha
            && (last.len() == 2
                || ["net", "com", "org", "tv", "bein", "info"].contains(&last.as_str()))
        {
            parts.pop();
        }
    }
    parts.join(" ")
}

/// Words that only describe the feed in guide ids (`beIN_SPORTS1_DIGITAL_Mono_AR`).
const GUIDE_NOISE: &[&str] = &[
    "digital", "mono", "stereo", "new", "logos", "com", "net", "www",
];

/// Aggressive key of a guide channel (id or display-name): feed words, a trailing `AR`/`EN` language
/// tag and repeated words dropped.
fn reduce(s: &str, is_id: bool) -> String {
    let src = if is_id { strip_tld(s) } else { s.to_string() };
    let mut t: Vec<String> = tokens_of(&src)
        .into_iter()
        .filter(|t| !NOISE.contains(&t.as_str()) && !GUIDE_NOISE.contains(&t.as_str()))
        .collect();
    if t.len() > 1 && t.last().is_some_and(|l| l == "ar" || l == "en") {
        t.pop();
    }
    let mut seen: Vec<String> = vec![];
    for x in t {
        if !seen.contains(&x) {
            seen.push(x);
        }
    }
    seen.join("")
}

/// Every key a guide channel can be found under: the names as written, the id, the reduced forms,
/// and (last) a number-less alias for `…1` channels (`beIN SPORTS XTRA` → `XTRA 1`).
pub fn guide_keys(id: &str, names: &[String]) -> Vec<String> {
    fn push(out: &mut Vec<String>, k: String) {
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    let mut out = vec![];
    for n in names {
        push(&mut out, norm_name(n));
    }
    push(&mut out, norm_id(id));
    let reduced: Vec<String> = names
        .iter()
        .map(|n| reduce(n, false))
        .chain(std::iter::once(reduce(id, true)))
        .collect();
    for r in &reduced {
        push(&mut out, r.clone());
    }
    for r in &reduced {
        if let Some(base) = r.strip_suffix('1') {
            if base.len() >= 3 && base.chars().last().is_some_and(char::is_alphabetic) {
                push(&mut out, base.to_string());
            }
        }
    }
    out
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

/// A channel of ours with every guide channel it could be linked to, best first: an exact
/// `tvg-id`, then name matches ordered by guide precedence (rank), then guide order.
pub struct ChanCands {
    pub id: i64,
    pub source_id: i64,
    pub enabled: bool,
    pub manual: bool,
    pub tvg_id: Option<String>,
    pub current: Option<String>,
    pub cands: Vec<String>,
}

pub async fn candidates(pool: &SqlitePool) -> Result<Vec<ChanCands>> {
    let by_id: HashMap<String, String> =
        sqlx::query_as::<_, (String,)>("SELECT id FROM epg_channels")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id,)| (id.to_lowercase(), id))
            .collect();
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (n, id) in sqlx::query_as::<_, (String, String)>(
        "SELECT n.norm_name, n.epg_id FROM epg_channel_names n \
         JOIN epg_channels c ON c.id = n.epg_id ORDER BY c.rank, n.rowid",
    )
    .fetch_all(pool)
    .await?
    {
        let v = by_name.entry(n).or_default();
        if !v.contains(&id) {
            v.push(id);
        }
    }
    type Row = (
        i64,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, source_id, name, tvg_id, tvg_name, epg_channel_id, epg_manual, enabled \
         FROM channels WHERE missing = 0",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, source_id, name, tvg_id, tvg_name, current, manual, enabled) in rows {
        let mut cands: Vec<String> = vec![];
        let mut add = |c: &String| {
            if !cands.contains(c) {
                cands.push(c.clone());
            }
        };
        if let Some(c) = tvg_id.as_deref().and_then(|t| by_id.get(&t.to_lowercase())) {
            add(c);
        }
        for key in [Some(norm_name(&name)), tvg_name.as_deref().map(norm_name)]
            .into_iter()
            .flatten()
        {
            if let Some(v) = by_name.get(&key) {
                v.iter().for_each(&mut add);
            }
        }
        out.push(ChanCands {
            id,
            source_id,
            enabled: enabled != 0,
            manual: manual != 0,
            tvg_id,
            current,
            cands,
        });
    }
    Ok(out)
}

/// Guide channel ids whose programmes are worth storing: every candidate of every enabled channel
/// (the final choice is made once all guides are in), plus tvg-ids and manual links.
pub async fn wanted_ids(pool: &SqlitePool) -> Result<std::collections::HashSet<String>> {
    let mut w = std::collections::HashSet::new();
    for c in candidates(pool).await?.into_iter().filter(|c| c.enabled) {
        w.extend(c.cands);
        w.extend(c.tvg_id);
        w.extend(c.current);
    }
    Ok(w)
}

/// Guide channels that were stored only as alternatives: name candidates of some channel that no
/// channel ended up using (neither as its mapping nor as its tvg-id).
pub async fn unused_candidates(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = candidates(pool).await?;
    let used: std::collections::HashSet<&String> = rows
        .iter()
        .flat_map(|c| c.current.iter().chain(c.tvg_id.iter()))
        .collect();
    let mut out: Vec<String> = rows
        .iter()
        .flat_map(|c| c.cands.iter())
        .filter(|id| !used.contains(id))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Recompute `channels.epg_channel_id` for channels without a manual override: among the
/// candidates, the first (best guide) that actually has programmes stored; with none stored yet,
/// the first. Returns the number of channels that now have a mapping.
pub async fn map_channels(pool: &SqlitePool) -> Result<usize> {
    let has_epg: i64 = sqlx::query_scalar("SELECT count(*) FROM epg_channels")
        .fetch_one(pool)
        .await?;
    if has_epg == 0 {
        return Ok(0);
    }
    let with_data: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT channel_tvg_id FROM epg_programs")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let rows = candidates(pool).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut mapped = 0;
    for c in rows.into_iter().filter(|c| !c.manual) {
        let found = c
            .cands
            .iter()
            .find(|x| with_data.contains(*x))
            .or_else(|| c.cands.first())
            .cloned();
        if found.is_some() {
            mapped += 1;
        }
        if found != c.current {
            sqlx::query("UPDATE channels SET epg_channel_id = ? WHERE id = ?")
                .bind(&found)
                .bind(c.id)
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
        assert_eq!(norm_name("AR: MBC 2 FHD HEVC M"), norm_name("MBC 2"));
        assert_eq!(
            norm_name("UK| Sky Sports Main Event UHD"),
            norm_name("Sky Sports Main Event")
        );
        assert_eq!(norm_name("AR| MBC 1 ᴴᴰ"), "mbc1");
        // short real words are not mistaken for country tags without a separator
        assert_eq!(norm_name("ESPN News"), "espnnews");
        assert_eq!(token_key("One BBC"), token_key("BBC One"));
    }

    #[test]
    fn epgshare_ids_match_provider_names() {
        // (guide id or display-name, provider channel name)
        for (guide, provider) in [
            ("MBC.2.ae", "AR: MBC 2 HD"),
            ("MBC.2.ae", "MBC 2 FHD"),
            ("MBC.2.ae", "AR| MBC 2 ᴴᴰ"),
            ("beIN.Sports.1.HD.qa", "beIN SPORTS 1 HD ᴿᴬᵂ"),
            ("beIN.Sports.1.HD.qa", "AR: beIN SPORTS 1 FHD"),
            ("Sky.Sports.Main.Event.uk", "UK| SKY SPORTS MAIN EVENT FHD"),
            ("SkySportsMainEvent.uk", "UK| SKY SPORTS MAIN EVENT FHD"),
            ("CNN.us", "US| CNN HD"),
            ("CNN.us", "CNN 4K HEVC"),
            ("Al.Jazeera.English.qa", "AR: Al Jazeera English H.265"),
        ] {
            assert_eq!(norm_id(guide), norm_name(provider), "{guide} vs {provider}");
        }
        assert_eq!(norm_id("MBC.2.ae"), "mbc2");
        // display names of such guides
        assert_eq!(
            norm_name("beIN Sports 1 HD"),
            norm_name("beIN SPORTS 1 HD ᴿᴬᵂ")
        );
        // plain ids without a TLD / with spaces are names
        assert_eq!(norm_id("BBC One"), "bbcone");
        assert_eq!(norm_id("cnn"), "cnn");
    }

    #[test]
    fn different_channels_and_timeshift_stay_distinct() {
        assert_ne!(norm_name("MBC 2"), norm_name("MBC 22"));
        assert_ne!(norm_name("MBC 2"), norm_name("MBC 4"));
        assert_ne!(norm_name("beIN SPORTS 1"), norm_name("beIN SPORTS 2"));
        assert_ne!(norm_name("Channel 4 +1"), norm_name("Channel 4"));
        assert_ne!(norm_name("Channel 4 +1"), norm_name("Channel 41"));
        assert_eq!(norm_name("UK: Channel 4 +1 HD"), norm_name("Channel.4.+1"));
        assert_eq!(norm_id("Channel4plus1.uk"), norm_id("Channel4plus1.uk"));
        assert_ne!(
            norm_name("Sky Sports Main Event"),
            norm_name("Sky Sports Premier League")
        );
    }

    #[test]
    fn arabic_and_unicode_names_fold() {
        // alef variants, ta marbuta, diacritics, Arabic-Indic digits
        assert_eq!(norm_name("أم بي سي ٢"), norm_name("ام بي سي 2"));
        assert_eq!(norm_name("قناة الجزيرة"), norm_name("AR: قناة الجزيره HD"));
        assert_eq!(norm_name("Café TV"), norm_name("cafe"));
        assert!(!norm_name("الجزيرة").is_empty());
    }
}

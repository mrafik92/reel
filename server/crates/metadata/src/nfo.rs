//! Kodi-style `.nfo` parsing (tiny tag extractor; no XML dependency).

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Nfo {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    pub plot: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    /// Content rating (`<mpaa>`), e.g. `PG-13`.
    pub mpaa: Option<String>,
    pub runtime_min: Option<i64>,
    /// `<premiered>` / `<aired>` / `<releasedate>` as `YYYY-MM-DD`.
    pub aired: Option<String>,
    /// `(name, role)` of `<actor>` entries, in file order.
    pub actors: Vec<(String, Option<String>)>,
    /// Episode nfo: `<season>` / `<episode>`.
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

fn unescape(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("<![CDATA[")
        .and_then(|x| x.strip_suffix("]]>"))
        .unwrap_or(s);
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// First `<tag ...>text</tag>`; returns (attributes, text).
fn find_tags<'a>(xml: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = vec![];
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        // must be followed by '>' or whitespace (not a longer tag name)
        match after.chars().next() {
            Some('>') | Some(' ') | Some('\t') | Some('\n') | Some('\r') => {}
            _ => {
                rest = after;
                continue;
            }
        }
        let Some(gt) = after.find('>') else { break };
        let attrs = &after[..gt];
        let body = &after[gt + 1..];
        let Some(end) = body.find(&close) else { break };
        out.push((attrs, &body[..end]));
        rest = &body[end + close.len()..];
    }
    out
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!("{name}=");
    let i = attrs.find(&pat)? + pat.len();
    let rest = &attrs[i..];
    let q = rest.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let rest = &rest[1..];
    Some(&rest[..rest.find(q)?])
}

pub fn parse(xml: &str) -> Nfo {
    let first = |t: &str| {
        find_tags(xml, t)
            .first()
            .map(|(_, b)| unescape(b))
            .filter(|s| !s.is_empty())
    };
    let mut n = Nfo {
        title: first("title"),
        year: first("year").and_then(|y| y.parse().ok()),
        plot: first("plot"),
        ..Default::default()
    };
    for (attrs, body) in find_tags(xml, "uniqueid") {
        let v = unescape(body);
        match attr(attrs, "type").map(str::to_ascii_lowercase).as_deref() {
            Some("tmdb") => n.tmdb_id = v.parse().ok(),
            Some("imdb") => n.imdb_id = Some(v),
            Some("tvdb") => n.tvdb_id = v.parse().ok(),
            _ => {}
        }
    }
    if n.tmdb_id.is_none() {
        n.tmdb_id = first("tmdbid").and_then(|v| v.parse().ok());
    }
    n.aired = first("aired")
        .or_else(|| first("premiered"))
        .or_else(|| first("releasedate"))
        .filter(|d| d.len() >= 10);
    if n.year.is_none() {
        n.year = n.aired.as_deref().and_then(|d| d.get(..4)?.parse().ok());
    }
    n.genres = find_tags(xml, "genre")
        .into_iter()
        .flat_map(|(_, b)| {
            unescape(b)
                .split('/')
                .map(|g| g.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|g| !g.is_empty())
        .collect();
    n.rating = find_tags(xml, "value")
        .first()
        .map(|(_, b)| unescape(b))
        .or_else(|| first("rating"))
        .and_then(|r| r.parse::<f64>().ok())
        .filter(|r| *r > 0.0);
    n.mpaa = first("mpaa").map(|m| m.trim_start_matches("Rated ").trim().to_string());
    n.runtime_min = first("runtime")
        .and_then(|r| r.parse().ok())
        .filter(|r: &i64| *r > 0);
    n.season = first("season").and_then(|v| v.parse().ok());
    n.episode = first("episode").and_then(|v| v.parse().ok());
    for (_, body) in find_tags(xml, "actor") {
        let name = find_tags(body, "name").first().map(|(_, b)| unescape(b));
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            let role = find_tags(body, "role")
                .first()
                .map(|(_, b)| unescape(b))
                .filter(|r| !r.is_empty());
            n.actors.push((name, role));
        }
    }
    n
}

/// Episode nfo files may hold several `<episodedetails>` blocks (multi-episode files).
pub fn parse_episodes(xml: &str) -> Vec<Nfo> {
    let blocks = find_tags(xml, "episodedetails");
    if blocks.is_empty() {
        return vec![parse(xml)];
    }
    blocks.into_iter().map(|(_, b)| parse(b)).collect()
}

/// Read an episode nfo (one `Nfo` per `<episodedetails>` block).
pub async fn read_episodes(path: &Path) -> Option<Vec<Nfo>> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    Some(parse_episodes(&text))
}

pub async fn read(path: &Path) -> Option<Nfo> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    Some(parse(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_movie_nfo() {
        let n = parse(
            r#"<?xml version="1.0"?><movie><title>Tom &amp; Jerry</title><year>2021</year>
            <uniqueid type="imdb" default="true">tt1</uniqueid><uniqueid type="tmdb">587807</uniqueid></movie>"#,
        );
        assert_eq!(n.title.as_deref(), Some("Tom & Jerry"));
        assert_eq!(n.year, Some(2021));
        assert_eq!(n.tmdb_id, Some(587807));
        assert_eq!(n.imdb_id.as_deref(), Some("tt1"));
    }

    #[test]
    fn parses_genres_rating_mpaa_runtime_and_cast() {
        let n = parse(
            r#"<movie><title>X</title><premiered>2021-09-15</premiered><runtime>155</runtime>
            <mpaa>Rated PG-13</mpaa><genre>Sci-Fi</genre><genre>Adventure / Drama</genre>
            <ratings><rating name="imdb"><value>7.8</value><votes>1</votes></rating></ratings>
            <actor><name>Zendaya</name><role>Chani</role></actor><actor><name>No Role</name></actor></movie>"#,
        );
        assert_eq!(n.year, Some(2021));
        assert_eq!(n.aired.as_deref(), Some("2021-09-15"));
        assert_eq!(n.runtime_min, Some(155));
        assert_eq!(n.mpaa.as_deref(), Some("PG-13"));
        assert_eq!(n.genres, ["Sci-Fi", "Adventure", "Drama"]);
        assert_eq!(n.rating, Some(7.8));
        assert_eq!(
            n.actors,
            vec![
                ("Zendaya".into(), Some("Chani".into())),
                ("No Role".into(), None)
            ]
        );
    }

    #[test]
    fn multi_episode_nfo_blocks() {
        let eps = parse_episodes(
            "<episodedetails><title>A</title><season>1</season><episode>2</episode></episodedetails>\
             <episodedetails><title>B</title><season>1</season><episode>3</episode><aired>2020-02-02</aired></episodedetails>",
        );
        assert_eq!(eps.len(), 2);
        assert_eq!(
            (
                eps[1].title.as_deref(),
                eps[1].episode,
                eps[1].aired.as_deref()
            ),
            (Some("B"), Some(3), Some("2020-02-02"))
        );
        assert_eq!(parse_episodes("<movie><title>Z</title></movie>").len(), 1);
    }

    #[test]
    fn titles_not_confused_with_originaltitle() {
        let n = parse("<movie><originaltitle>X</originaltitle><title>Y</title></movie>");
        assert_eq!(n.title.as_deref(), Some("Y"));
    }
}

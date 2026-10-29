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
    if n.year.is_none() {
        n.year = first("premiered")
            .or_else(|| first("releasedate"))
            .and_then(|d| d.get(..4)?.parse().ok());
    }
    n
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
    fn titles_not_confused_with_originaltitle() {
        let n = parse("<movie><originaltitle>X</originaltitle><title>Y</title></movie>");
        assert_eq!(n.title.as_deref(), Some("Y"));
    }
}

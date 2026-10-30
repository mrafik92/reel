//! Incremental M3U / M3U8 playlist parser.
//!
//! `#EXTINF` attributes (`tvg-id`, `tvg-name`, `tvg-logo`, `group-title`, `tvg-chno`, `catchup*`),
//! `#EXTGRP`, `#EXTVLCOPT` (user-agent / referrer) and Kodi-style `|User-Agent=…` URL suffixes.
//! Feed it chunks as they arrive from the network; entries are emitted as soon as their URL line
//! is complete.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catchup {
    /// `default`, `append`, `shift`, `flussonic`, `xc`, …
    pub kind: Option<String>,
    pub days: Option<u32>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct M3uEntry {
    pub name: String,
    pub url: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub logo: Option<String>,
    pub group: Option<String>,
    pub number: Option<i64>,
    pub catchup: Option<Catchup>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Default)]
pub struct M3uParser {
    buf: Vec<u8>,
    pending: Option<M3uEntry>,
    started: bool,
    /// EPG URL advertised by the `#EXTM3U` header (`url-tvg` / `x-tvg-url`).
    pub epg_url: Option<String>,
}

/// Parse `key="value"` / `key=value` pairs.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        let key = s[ks..i].to_ascii_lowercase();
        i += 1;
        let val = if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            let q = b[i];
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            let v = &s[vs..i.min(b.len())];
            i += 1;
            v
        } else {
            let vs = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            &s[vs..i]
        };
        out.push((key, val.trim().to_string()));
    }
    out
}

/// Split `-1 tvg-id="a,b" … ,Name` into (attribute text, name) at the first unquoted comma.
fn split_extinf(rest: &str) -> (&str, &str) {
    let mut quote: Option<char> = None;
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (None, '"') | (None, '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, ',') => return (&rest[..i], rest[i + 1..].trim()),
            _ => {}
        }
    }
    (rest, "")
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

impl M3uParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; completed entries are appended to `out`.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<M3uEntry>) {
        self.buf.extend_from_slice(chunk);
        let mut start = 0;
        while let Some(nl) = self.buf[start..].iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&self.buf[start..start + nl]).into_owned();
            start += nl + 1;
            self.line(&line, out);
        }
        self.buf.drain(..start);
    }

    /// Flush a trailing line without newline.
    pub fn finish(&mut self, out: &mut Vec<M3uEntry>) {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            self.line(&line, out);
        }
    }

    fn line(&mut self, raw: &str, out: &mut Vec<M3uEntry>) {
        let mut line = raw.trim_end_matches(['\r', '\n']).trim();
        if !self.started {
            line = line.trim_start_matches('\u{feff}');
        }
        if line.is_empty() {
            return;
        }
        if let Some(rest) = line.strip_prefix("#EXTM3U") {
            self.started = true;
            for (k, v) in parse_attrs(rest) {
                if matches!(k.as_str(), "url-tvg" | "x-tvg-url" | "tvg-url")
                    && self.epg_url.is_none()
                {
                    self.epg_url = v.split(',').next().and_then(non_empty);
                }
            }
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let (head, name) = split_extinf(rest);
            // The duration is the first token; attributes follow.
            let attrs_text = head.split_once(char::is_whitespace).map_or("", |x| x.1);
            let mut e = M3uEntry {
                name: name.to_string(),
                ..Default::default()
            };
            let mut catchup = Catchup::default();
            for (k, v) in parse_attrs(attrs_text) {
                match k.as_str() {
                    "tvg-id" => e.tvg_id = non_empty(&v),
                    "tvg-name" => e.tvg_name = non_empty(&v),
                    "tvg-logo" | "logo" => e.logo = non_empty(&v),
                    "group-title" => e.group = non_empty(&v),
                    "tvg-chno" | "channel-number" => e.number = v.trim().parse().ok(),
                    "catchup" | "catchup-type" => catchup.kind = non_empty(&v),
                    "catchup-days" | "timeshift" | "tvg-rec" => {
                        catchup.days = v.trim().parse().ok()
                    }
                    "catchup-source" => catchup.source = non_empty(&v),
                    "user-agent" | "http-user-agent" => e.user_agent = non_empty(&v),
                    "referer" | "referrer" | "http-referrer" => e.referrer = non_empty(&v),
                    _ => {}
                }
            }
            if catchup != Catchup::default() {
                e.catchup = Some(catchup);
            }
            if e.name.is_empty() {
                e.name = e.tvg_name.clone().unwrap_or_default();
            }
            self.pending = Some(e);
        } else if let Some(g) = line.strip_prefix("#EXTGRP:") {
            if let Some(e) = self.pending.as_mut() {
                if e.group.is_none() {
                    e.group = non_empty(g);
                }
            }
        } else if let Some(opt) = line.strip_prefix("#EXTVLCOPT:") {
            if let (Some(e), Some((k, v))) = (self.pending.as_mut(), opt.split_once('=')) {
                match k.trim().to_ascii_lowercase().as_str() {
                    "http-user-agent" => e.user_agent = non_empty(v),
                    "http-referrer" | "http-referer" => e.referrer = non_empty(v),
                    _ => {}
                }
            }
        } else if line.starts_with('#') {
            // other directives (#EXT-X-*, #KODIPROP …) are ignored
        } else if let Some(mut e) = self.pending.take() {
            // Kodi-style header suffix: url|User-Agent=x&Referer=y
            let (url, suffix) = match line.split_once('|') {
                Some((u, s)) => (u, Some(s)),
                None => (line, None),
            };
            if let Some(s) = suffix {
                for kv in s.split('&') {
                    if let Some((k, v)) = kv.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "user-agent" => e.user_agent = non_empty(v),
                            "referer" | "referrer" => e.referrer = non_empty(v),
                            _ => {}
                        }
                    }
                }
            }
            e.url = url.trim().to_string();
            if e.name.is_empty() {
                e.name = e.url.clone();
            }
            out.push(e);
        }
    }
}

/// Parse a whole playlist (tests / small inputs).
pub fn parse(text: &str) -> (Vec<M3uEntry>, Option<String>) {
    let mut p = M3uParser::new();
    let mut out = Vec::new();
    p.feed(text.as_bytes(), &mut out);
    p.finish(&mut out);
    (out, p.epg_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}#EXTM3U url-tvg=\"http://epg.example/guide.xml.gz,http://b\" x-foo=1\r\n\
#EXTINF:-1 tvg-id=\"bbc1.uk\" tvg-name=\"BBC One HD\" tvg-logo=\"http://l/bbc.png\" group-title=\"UK, News\" tvg-chno=\"101\" catchup=\"shift\" catchup-days=\"3\" catchup-source=\"http://x/{utc}\",BBC One HD\r\n\
http://provider/live/1.ts\r\n\
#EXTINF:-1 group-title=\"Sports\",Sky Sports, Main Event\n\
#EXTGRP:Ignored\n\
#EXTVLCOPT:http-user-agent=VLC/3.0\n\
#EXTVLCOPT:http-referrer=http://ref.example/\n\
http://provider/live/2.m3u8\n\
#EXTINF:-1 tvg-id=\"\",Plain\n\
#KODIPROP:inputstream=x\n\
http://p/3.ts|User-Agent=Foo/1&Referer=http://r/\n\
#EXTINF:-1,Last without newline\n\
http://p/4";

    #[test]
    fn parses_attributes_groups_and_headers() {
        let (e, epg) = parse(SAMPLE);
        assert_eq!(epg.as_deref(), Some("http://epg.example/guide.xml.gz"));
        assert_eq!(e.len(), 4);
        let a = &e[0];
        assert_eq!(a.name, "BBC One HD");
        assert_eq!(a.url, "http://provider/live/1.ts");
        assert_eq!(a.tvg_id.as_deref(), Some("bbc1.uk"));
        assert_eq!(a.tvg_name.as_deref(), Some("BBC One HD"));
        assert_eq!(a.logo.as_deref(), Some("http://l/bbc.png"));
        assert_eq!(a.group.as_deref(), Some("UK, News"), "comma inside quotes");
        assert_eq!(a.number, Some(101));
        assert_eq!(
            a.catchup,
            Some(Catchup {
                kind: Some("shift".into()),
                days: Some(3),
                source: Some("http://x/{utc}".into())
            })
        );
        let b = &e[1];
        assert_eq!(b.name, "Sky Sports, Main Event", "name keeps later commas");
        assert_eq!(
            b.group.as_deref(),
            Some("Sports"),
            "EXTGRP does not override"
        );
        assert_eq!(b.user_agent.as_deref(), Some("VLC/3.0"));
        assert_eq!(b.referrer.as_deref(), Some("http://ref.example/"));
        let c = &e[2];
        assert_eq!(c.tvg_id, None, "empty attribute is None");
        assert_eq!(c.url, "http://p/3.ts");
        assert_eq!(c.user_agent.as_deref(), Some("Foo/1"));
        assert_eq!(c.referrer.as_deref(), Some("http://r/"));
        assert_eq!(e[3].url, "http://p/4");
    }

    #[test]
    fn streaming_in_odd_chunks_matches_whole_parse() {
        let whole = parse(SAMPLE).0;
        let mut p = M3uParser::new();
        let mut out = vec![];
        for chunk in SAMPLE.as_bytes().chunks(7) {
            p.feed(chunk, &mut out);
        }
        p.finish(&mut out);
        assert_eq!(out, whole);
    }

    #[test]
    fn group_from_extgrp_when_missing() {
        let (e, _) = parse("#EXTM3U\n#EXTINF:-1,X\n#EXTGRP:Movies\nhttp://a/1\n");
        assert_eq!(e[0].group.as_deref(), Some("Movies"));
    }
}

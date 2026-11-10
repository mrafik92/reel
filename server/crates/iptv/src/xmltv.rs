//! Streaming XMLTV parser (quick-xml). Handles gzip transparently, keeps only programmes inside a
//! time window, and hands results out in batches so memory stays flat for 100 MB+ guides.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use quick_xml::{
    Reader, XmlVersion,
    escape::resolve_predefined_entity,
    events::{BytesRef, Event},
};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct XmlChannel {
    pub id: String,
    pub names: Vec<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Programme {
    pub channel: String,
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub sub_title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    pub episode_num: Option<String>,
}

#[derive(Debug)]
pub enum XmlMsg {
    Channel(XmlChannel),
    Programmes(Vec<Programme>),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ParseStats {
    pub channels: usize,
    pub programmes_seen: usize,
    pub programmes_kept: usize,
    /// Earliest start / latest stop of ALL programmes (window or not): tells a stale guide apart.
    pub oldest_start: Option<i64>,
    pub newest_stop: Option<i64>,
}

/// Days between 1970-01-01 and a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DD` (UTC) of a unix time.
pub fn fmt_date(ts: i64) -> String {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `20240131203000 +0100` (zone optional, seconds optional) → unix seconds.
pub fn parse_xmltv_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 12 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| digits.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi) = (n(0..4)?, n(4..6)?, n(6..8)?, n(8..10)?, n(10..12)?);
    let sec = if digits.len() >= 14 { n(12..14)? } else { 0 };
    let mut t = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec;
    let tz = s[digits.len()..].trim();
    if let Some(sign) = tz.chars().next().filter(|c| *c == '+' || *c == '-') {
        let z: String = tz[1..].chars().filter(char::is_ascii_digit).collect();
        if z.len() >= 4 {
            let off = z[0..2].parse::<i64>().ok()? * 3600 + z[2..4].parse::<i64>().ok()? * 60;
            t -= if sign == '+' { off } else { -off };
        }
    }
    Some(t)
}

/// Open a file as a buffered reader, transparently gunzipping (detected by magic bytes).
pub fn open_maybe_gz(path: &Path) -> Result<Box<dyn BufRead + Send>> {
    let mut f = File::open(path)?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    f.seek(SeekFrom::Start(0))?;
    if n == 2 && magic == [0x1F, 0x8B] {
        Ok(Box::new(BufReader::with_capacity(
            128 * 1024,
            flate2::read::MultiGzDecoder::new(BufReader::new(f)),
        )))
    } else {
        Ok(Box::new(BufReader::with_capacity(128 * 1024, f)))
    }
}

/// Cheap sanity check of a downloaded guide before parsing: it must be (gzipped) XML. Providers
/// answer with HTTP 200 and an HTML error page / empty body / JSON for bad credentials or UAs.
pub fn sniff_guide(path: &Path) -> Result<()> {
    let mut r =
        open_maybe_gz(path).map_err(|e| Error::Parse(format!("guide is not readable ({e})")))?;
    let mut head = vec![0u8; 2048];
    let mut n = 0;
    while n < head.len() {
        match r.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => {
                return Err(Error::Parse(format!(
                    "guide is not valid gzip/XML data ({e})"
                )));
            }
        }
    }
    head.truncate(n);
    let text = String::from_utf8_lossy(&head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.is_empty() {
        return Err(Error::Parse(
            "the guide download was empty (wrong credentials or the provider blocked the request?)"
                .into(),
        ));
    }
    let lower = t.to_ascii_lowercase();
    let is_xml =
        lower.starts_with("<?xml") || lower.starts_with("<tv") || lower.starts_with("<!doctype tv");
    if !is_xml {
        let kind = if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
            "an HTML page"
        } else if lower.starts_with('{') || lower.starts_with('[') {
            "JSON"
        } else {
            "non-XML data"
        };
        let preview: String = t
            .chars()
            .take(80)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        return Err(Error::Parse(format!(
            "the guide URL returned {kind} instead of XMLTV (starts with: {})",
            crate::net::redact_text(preview.trim())
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    None,
    DisplayName,
    Title,
    SubTitle,
    Desc,
    Category,
    EpisodeNum,
}

/// Parse `reader`; `window` = (earliest stop, latest start) in unix seconds.
/// Append the text an entity/character reference (`&amp;`, `&#233;`) stands for. Unknown named
/// entities (no DTD support) are kept verbatim instead of failing the whole guide.
fn push_ref(text: &mut String, r: &BytesRef<'_>) -> std::result::Result<(), quick_xml::Error> {
    if let Some(c) = r.resolve_char_ref()? {
        text.push(c);
    } else if let Some(v) = resolve_predefined_entity(r) {
        text.push_str(v);
    } else {
        text.push('&');
        text.push_str(r);
        text.push(';');
    }
    Ok(())
}

/// `sink` receives messages and returns `false` to abort. Programmes are batched `batch` at a time.
pub fn parse_reader<R: BufRead>(
    reader: R,
    window: (i64, i64),
    batch: usize,
    mut sink: impl FnMut(XmlMsg) -> bool,
) -> Result<ParseStats> {
    // No reader-level trim_text: quick-xml reports `&amp;` etc. as separate `GeneralRef`
    // events, so trimming each text chunk would eat the spaces around them
    // ("News &amp; Weather" → "News&Weather"). Field text is trimmed once at its end tag.
    let mut rd = Reader::from_reader(reader);
    let mut buf = Vec::new();
    let mut stats = ParseStats::default();
    let mut out: Vec<Programme> = Vec::with_capacity(batch);
    let mut chan: Option<XmlChannel> = None;
    let mut prog: Option<Programme> = None;
    let mut field = Field::None;
    let mut text = String::new();

    let attr = |e: &quick_xml::events::BytesStart<'_>, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .and_then(|a| {
                a.normalized_value(XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
    };

    loop {
        buf.clear();
        let ev = rd
            .read_event_into(&mut buf)
            .map_err(|e| Error::Parse(format!("xmltv: {e}")))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(ev, Event::Empty(_));
                match e.name().as_ref() {
                    "channel" => {
                        chan = attr(e, "id").map(|id| XmlChannel {
                            id,
                            names: vec![],
                            icon: None,
                        });
                        if is_empty {
                            chan = None;
                        }
                    }
                    "programme" => {
                        stats.programmes_seen += 1;
                        let start = attr(e, "start").and_then(|s| parse_xmltv_time(&s));
                        let stop = attr(e, "stop").and_then(|s| parse_xmltv_time(&s));
                        if let Some(s) = start {
                            stats.oldest_start = Some(stats.oldest_start.map_or(s, |o| o.min(s)));
                        }
                        if let Some(t) = stop {
                            stats.newest_stop = Some(stats.newest_stop.map_or(t, |o| o.max(t)));
                        }
                        prog = match (attr(e, "channel"), start, stop) {
                            (Some(channel), Some(start), Some(stop))
                                if stop >= window.0 && start <= window.1 && !is_empty =>
                            {
                                Some(Programme {
                                    channel,
                                    start,
                                    stop,
                                    title: String::new(),
                                    sub_title: None,
                                    description: None,
                                    category: None,
                                    icon: None,
                                    episode_num: None,
                                })
                            }
                            _ => None,
                        };
                    }
                    "icon" => {
                        if let Some(src) = attr(e, "src") {
                            if let Some(p) = prog.as_mut() {
                                p.icon.get_or_insert(src);
                            } else if let Some(c) = chan.as_mut() {
                                c.icon.get_or_insert(src);
                            }
                        }
                    }
                    n => {
                        field = match (n, chan.is_some(), prog.is_some()) {
                            ("display-name", true, _) => Field::DisplayName,
                            ("title", _, true) => Field::Title,
                            ("sub-title", _, true) => Field::SubTitle,
                            ("desc", _, true) => Field::Desc,
                            ("category", _, true) => Field::Category,
                            ("episode-num", _, true) => Field::EpisodeNum,
                            _ => Field::None,
                        };
                        text.clear();
                    }
                }
            }
            Event::Text(ref t) if field != Field::None => {
                text.push_str(&t.xml10_content());
            }
            Event::GeneralRef(ref r) if field != Field::None => {
                push_ref(&mut text, r).map_err(|e| Error::Parse(format!("xmltv: {e}")))?;
            }
            Event::CData(ref t) if field != Field::None => {
                text.push_str(&t.xml10_content());
            }
            Event::End(ref e) => match e.name().as_ref() {
                "channel" => {
                    if let Some(c) = chan.take() {
                        stats.channels += 1;
                        if !sink(XmlMsg::Channel(c)) {
                            return Ok(stats);
                        }
                    }
                }
                "programme" => {
                    if let Some(p) = prog.take() {
                        if !p.title.is_empty() {
                            stats.programmes_kept += 1;
                            out.push(p);
                            if out.len() >= batch
                                && !sink(XmlMsg::Programmes(std::mem::replace(
                                    &mut out,
                                    Vec::with_capacity(batch),
                                )))
                            {
                                return Ok(stats);
                            }
                        }
                    }
                }
                _ => {
                    let v = text.trim().to_string();
                    if !v.is_empty() {
                        match field {
                            Field::DisplayName => {
                                if let Some(c) = chan.as_mut() {
                                    c.names.push(v);
                                }
                            }
                            Field::Title => {
                                if let Some(p) = prog.as_mut() {
                                    if p.title.is_empty() {
                                        p.title = v;
                                    }
                                }
                            }
                            Field::SubTitle => {
                                if let Some(p) = prog.as_mut() {
                                    p.sub_title.get_or_insert(v);
                                }
                            }
                            Field::Desc => {
                                if let Some(p) = prog.as_mut() {
                                    p.description.get_or_insert(v);
                                }
                            }
                            Field::Category => {
                                if let Some(p) = prog.as_mut() {
                                    p.category.get_or_insert(v);
                                }
                            }
                            Field::EpisodeNum => {
                                if let Some(p) = prog.as_mut() {
                                    p.episode_num.get_or_insert(v);
                                }
                            }
                            Field::None => {}
                        }
                    }
                    field = Field::None;
                    text.clear();
                }
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if !out.is_empty() {
        sink(XmlMsg::Programmes(out));
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn times_and_zones() {
        assert_eq!(parse_xmltv_time("19700101000000 +0000"), Some(0));
        assert_eq!(
            parse_xmltv_time("20240101120000 +0000"),
            Some(1_704_110_400)
        );
        assert_eq!(
            parse_xmltv_time("20240101130000 +0100"),
            Some(1_704_110_400)
        );
        assert_eq!(
            parse_xmltv_time("20240101073000 -0500"),
            Some(1_704_112_200)
        );
        assert_eq!(parse_xmltv_time("202401011200"), Some(1_704_110_400));
        assert_eq!(parse_xmltv_time("2024"), None);
    }

    fn sample() -> String {
        // window of the test: 2024-01-01 12:00 ± a few hours
        r#"<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="t">
  <channel id="bbc1.uk"><display-name>BBC One</display-name><display-name lang="en">BBC 1 HD</display-name><icon src="http://i/bbc.png"/></channel>
  <channel id="cnn.us"><display-name>CNN</display-name></channel>
  <programme start="20240101110000 +0000" stop="20240101120000 +0000" channel="bbc1.uk">
    <title lang="en">News &amp; Weather</title><sub-title>Morning</sub-title>
    <desc><![CDATA[All the <b>news</b>]]></desc><category>News</category><category>Weather</category>
    <icon src="http://i/p.png"/><episode-num system="onscreen">S1E2</episode-num>
  </programme>
  <programme start="20240101120000 +0000" stop="20240101130000 +0000" channel="bbc1.uk"><title>Show</title></programme>
  <programme start="20230101120000 +0000" stop="20230101130000 +0000" channel="bbc1.uk"><title>Ancient</title></programme>
  <programme start="20240601120000 +0000" stop="20240601130000 +0000" channel="cnn.us"><title>Far future</title></programme>
  <programme start="20240101120000 +0000" stop="20240101123000 +0000" channel="cnn.us"><title></title></programme>
  <programme start="20240101123000 +0000" stop="20240101130000 +0000" channel="cnn.us"><title>Anderson Cooper</title></programme>
</tv>"#
            .to_string()
    }

    fn run(
        reader: impl BufRead,
        batch: usize,
    ) -> (ParseStats, Vec<XmlChannel>, Vec<Programme>, usize) {
        let w = (
            parse_xmltv_time("20240101000000 +0000").unwrap(),
            parse_xmltv_time("20240108000000 +0000").unwrap(),
        );
        let (mut ch, mut pr, mut batches) = (vec![], vec![], 0);
        let st = parse_reader(reader, w, batch, |m| {
            match m {
                XmlMsg::Channel(c) => ch.push(c),
                XmlMsg::Programmes(p) => {
                    batches += 1;
                    pr.extend(p)
                }
            }
            true
        })
        .unwrap();
        (st, ch, pr, batches)
    }

    #[test]
    fn parses_channels_programmes_and_window() {
        let (st, ch, pr, batches) = run(Cursor::new(sample()), 2);
        assert_eq!(st.channels, 2);
        assert_eq!(ch[0].id, "bbc1.uk");
        assert_eq!(ch[0].names, vec!["BBC One", "BBC 1 HD"]);
        assert_eq!(ch[0].icon.as_deref(), Some("http://i/bbc.png"));
        assert_eq!(st.programmes_seen, 6);
        assert_eq!(st.programmes_kept, 3, "window + empty title filtered");
        assert_eq!(batches, 2, "batched by 2, remainder flushed");
        let p = &pr[0];
        assert_eq!(p.title, "News & Weather");
        assert_eq!(p.sub_title.as_deref(), Some("Morning"));
        assert_eq!(p.description.as_deref(), Some("All the <b>news</b>"));
        assert_eq!(p.category.as_deref(), Some("News"), "first category");
        assert_eq!(p.icon.as_deref(), Some("http://i/p.png"));
        assert_eq!(p.episode_num.as_deref(), Some("S1E2"));
        assert_eq!(p.start, 1_704_106_800);
        assert_eq!(pr[2].channel, "cnn.us");
    }

    #[test]
    fn gzip_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("a.xml");
        std::fs::write(&plain, sample()).unwrap();
        let gz = dir.path().join("a.bin"); // extension is irrelevant
        let mut enc =
            flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::fast());
        enc.write_all(sample().as_bytes()).unwrap();
        enc.finish().unwrap();
        let a = run(open_maybe_gz(&plain).unwrap(), 100);
        let b = run(open_maybe_gz(&gz).unwrap(), 100);
        assert_eq!(a.2, b.2);
        assert_eq!(b.0.programmes_kept, 3);
    }

    #[test]
    fn references_keep_surrounding_spaces() {
        // quick-xml ≥ 0.38 reports references as separate events; the spaces around them and
        // character references must survive, and unknown entities must not abort the guide.
        let xml = r#"<tv><channel id="a&amp;b"><display-name>Tom &amp; Jerry &#233;t&#xE9;</display-name></channel>
<programme start="20240101120000 +0000" stop="20240101130000 +0000" channel="a&amp;b">
  <title> Fish &lt; Chips &nbsp; </title><desc>
    Line one
    &quot;two&quot; </desc></programme></tv>"#;
        let (st, ch, pr, _) = run(Cursor::new(xml), 10);
        assert_eq!(st.channels, 1);
        assert_eq!(ch[0].id, "a&b");
        assert_eq!(ch[0].names, vec!["Tom & Jerry été"]);
        assert_eq!(pr[0].channel, "a&b");
        assert_eq!(pr[0].title, "Fish < Chips &nbsp;");
        assert_eq!(pr[0].description.as_deref(), Some("Line one\n    \"two\""));
    }

    #[test]
    fn many_attributes_parse_quickly() {
        // RUSTSEC-2026-0194: quick-xml < 0.41 checked duplicate attribute names in O(n²).
        // `id` comes last so the lookup walks (and duplicate-checks) every attribute.
        let attrs: String = (0..20_000).map(|i| format!(" a{i}=\"x\"")).collect();
        let xml =
            format!(r#"<tv><channel{attrs} id="c"><display-name>C</display-name></channel></tv>"#);
        let t = std::time::Instant::now();
        let (st, ch, _, _) = run(Cursor::new(xml), 10);
        assert_eq!(st.channels, 1);
        assert_eq!(ch[0].id, "c");
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }
}

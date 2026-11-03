//! Redaction of text that may carry provider credentials, URLs or server paths (ffmpeg stderr,
//! error details). Nothing produced here is ever meant to reach a client unredacted, but all
//! subprocess output is passed through this *before* it is stored or logged.

const SECRET_KEYS: &[&str] = &[
    "username", "password", "user", "pass", "pwd", "token", "api_key", "apikey", "key", "secret",
    "auth",
];

fn is_url_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | '(' | '`' | '\\')
}

/// Replace every `scheme://…` token by `<url>`.
fn strip_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        // scheme = run of [A-Za-z0-9+.-] before "://"
        let head = &rest[..i];
        let scheme_start = head
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
            .last()
            .map(|(p, _)| p)
            .unwrap_or(i);
        out.push_str(&rest[..scheme_start]);
        out.push_str("<url>");
        let after = &rest[i + 3..];
        let end = after.find(is_url_end).unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// `/live/USER/PASS/…` → `/live/***/***/…`
fn mask_xtream(s: &str) -> String {
    let mut out = s.to_string();
    for kind in ["live", "movie", "series", "timeshift"] {
        let pat = format!("/{kind}/");
        let mut search_from = 0;
        while let Some(rel) = out[search_from..].find(&pat) {
            let start = search_from + rel + pat.len();
            let tail = &out[start..];
            let mut parts = tail.splitn(3, '/');
            let (a, b, c) = (parts.next(), parts.next(), parts.next());
            if let (Some(a), Some(b), Some(_)) = (a, b, c) {
                if !a.is_empty()
                    && !b.is_empty()
                    && !a.contains(char::is_whitespace)
                    && !b.contains(char::is_whitespace)
                {
                    let end = start + a.len() + 1 + b.len();
                    out.replace_range(start..end, "***/***");
                    search_from = start + 7;
                    continue;
                }
            }
            search_from = start;
        }
    }
    out
}

fn mask_query_secrets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let mut i = 0;
    'outer: while i < s.len() {
        for k in SECRET_KEYS {
            if lower[i..].starts_with(k) && lower[i + k.len()..].starts_with('=') {
                let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                if prev_ok {
                    let vstart = i + k.len() + 1;
                    let vend = s[vstart..]
                        .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ','))
                        .map_or(s.len(), |p| vstart + p);
                    out.push_str(&s[i..vstart]);
                    out.push_str("***");
                    i = vend;
                    continue 'outer;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Absolute unix paths → `…/basename`.
fn shorten_paths(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.starts_with('/') && word[1..].contains('/') {
            let trimmed = word.trim_end_matches(['.', ',', ';', ':']);
            let tail: &str = &word[trimmed.len()..];
            let base = trimmed.rsplit('/').next().unwrap_or("");
            out.push_str("…/");
            out.push_str(base);
            out.push_str(tail);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in s.chars() {
        if c.is_whitespace() || matches!(c, '"' | '\'' | '=' | '[' | ']' | '(' | ')') {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Remove URLs, credentials and absolute paths from `s`.
pub fn redact_text(s: &str) -> String {
    shorten_paths(&mask_query_secrets(&mask_xtream(&strip_urls(s))))
}

/// Last path component (for file names in messages).
pub fn basename(p: &str) -> &str {
    p.trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_urls_credentials_and_paths() {
        let s = "[http @ 0x1] HTTP error 403 Forbidden for http://user:pw@prov.example:8080/movie/bob/hunter2/123.mkv?token=abc&x=1 while opening /data/media/Movies/Secret Film (2020)/f.mkv";
        let r = redact_text(s);
        for bad in [
            "://",
            "bob",
            "hunter2",
            "user:pw",
            "prov.example",
            "token=abc",
            "/data/media",
            "Movies/",
        ] {
            assert!(!r.contains(bad), "{bad} leaked: {r}");
        }
        assert!(r.contains("<url>"));
    }

    #[test]
    fn masks_bare_secrets_and_xtream_paths() {
        let r =
            redact_text("GET /live/bob/hunter2/55.ts?username=bob&password=hunter2&ok=1 failed");
        assert!(!r.contains("bob") && !r.contains("hunter2"), "{r}");
        assert!(r.contains("ok=1"));
        assert_eq!(
            redact_text("Invalid data found when processing input"),
            "Invalid data found when processing input"
        );
    }

    #[test]
    fn paths_keep_basename_only() {
        let r = redact_text("Error opening input file /mnt/nfs/library/Film.mkv.");
        assert_eq!(r, "Error opening input file …/Film.mkv.");
        assert_eq!(basename("/a/b/c.srt"), "c.srt");
    }
}

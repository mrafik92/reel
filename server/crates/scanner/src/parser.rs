//! Pure filename / path parser (no I/O).
//!
//! Input is a path *relative to the library root* (absolute paths work too, but
//! every parent directory is then considered a candidate show/movie folder).
//! Both `/` and `\` separators are accepted.

use std::sync::LazyLock;

use regex::Regex;

/// Library kind hint (mirrors `libraries.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Movies,
    Shows,
}

impl LibraryKind {
    /// Parse the DB value (`movies` / `shows`).
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "movies" => Some(Self::Movies),
            "shows" => Some(Self::Shows),
            _ => None,
        }
    }
}

/// What a video file represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Movie,
    Episode,
}

/// Parse result for a video file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMedia {
    pub kind: MediaKind,
    /// Movie title, or show title for episodes.
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub season: Option<i32>,
    /// Episode numbers (several for `S01E02E03`); empty for date-based episodes.
    pub episodes: Vec<i32>,
    /// `YYYY-MM-DD` for date-based episodes.
    pub air_date: Option<String>,
    pub episode_title: Option<String>,
    pub edition: Option<String>,
    /// Multi-part number (`cd1`, `part2`, ...).
    pub part: Option<i32>,
}

/// Sidecar subtitle info parsed from e.g. `Movie.en.forced.srt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSubtitle {
    /// File stem with language/flag tokens removed (`Movie`).
    pub base_stem: String,
    /// Full stem (`Movie.en.forced`).
    pub full_stem: String,
    pub language: Option<String>,
    pub forced: bool,
    pub default: bool,
    pub sdh: bool,
    pub ext: String,
}

/// Classification of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass {
    Video(ParsedMedia),
    Subtitle(SidecarSubtitle),
    Ignored,
}

pub const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "ts", "m2ts", "mts", "mpg", "mpeg", "webm", "flv",
    "ogv", "vob", "divx",
];
pub const SUBTITLE_EXTS: &[&str] = &["srt", "ass", "ssa", "sub", "vtt", "sup"];

const IGNORED_DIRS: &[&str] = &[
    "sample",
    "samples",
    "extras",
    "extra",
    "trailers",
    "trailer",
    "featurettes",
    "behind the scenes",
    "behindthescenes",
    "deleted scenes",
    "deletedscenes",
    "interviews",
    "scenes",
    "shorts",
    "other",
    "@eadir",
    "#recycle",
];

const IGNORED_SUFFIXES: &[&str] = &[
    "-trailer",
    ".trailer",
    " trailer",
    "-sample",
    ".sample",
    " sample",
    "-scene",
    "-clip",
    "-interview",
    "-behindthescenes",
    "-deleted",
    "-featurette",
    "-short",
    "-other",
];

/// True if a directory name should be skipped entirely while walking.
pub fn is_ignored_dir(name: &str) -> bool {
    let l = name.to_lowercase();
    l.starts_with('.') || IGNORED_DIRS.contains(&l.as_str())
}

/// Lower-cased extension of a file name, if any.
pub fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    Some(ext.to_lowercase())
}

pub fn is_video_ext(ext: &str) -> bool {
    VIDEO_EXTS.contains(&ext)
}

pub fn is_subtitle_ext(ext: &str) -> bool {
    SUBTITLE_EXTS.contains(&ext)
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(
    TMDB_RE,
    r"(?i)[\{\[\(]\s*tmdb(?:id)?\s*[-=: ]\s*(\d+)\s*[\}\]\)]"
);
re!(
    OTHER_ID_RE,
    r"(?i)[\{\[\(]\s*(?:imdb|tvdb)(?:id)?\s*[-=: ]\s*(?:tt)?\d+\s*[\}\]\)]"
);
re!(EDITION_TAG_RE, r"(?i)\{edition-([^}]*)\}");
re!(
    PART_SUFFIX_RE,
    r"(?i)[ ._-]+(?:cd|disc|disk|dvd|part|pt)[ ._-]?(\d{1,2})$"
);
re!(
    PART_DIR_RE,
    r"(?i)^(?:cd|disc|disk|dvd|part|pt)[ ._-]?(\d{1,2})$"
);
re!(
    QUALITY_RE,
    r"(?i)\b(?:480p|576p|720p|1080p|1080i|2160p|4k|uhd|blu-?ray|bdrip|brrip|bdremux|web-?dl|web-?rip|hdtv|pdtv|dvdrip|dvd-?r|hdrip|remux|x26[45]|h\.?26[45]|hevc|avc|xvid|divx|hdr10\+?|hdr|dovi|10bit|8bit|aac\d?|ac3|eac3|dts(?:-hd)?|truehd|atmos|flac|dd[p+]?[257]|proper|repack|internal)\b"
);
re!(YEAR4_RE, r"(?:19|20)\d{2}");
re!(LEADING_GROUP_RE, r"^\[[^\]]*\]\s*");
re!(BRACKETS_RE, r"\[[^\]]*\]");
re!(
    SEASON_DIR_RE,
    r"(?i)^(?:season|series|staffel|saison|temporada|s)[ ._-]?(\d{1,4})$"
);
re!(
    SXXEXX_RE,
    r"(?i)\bs(\d{1,4})[ ]?e(\d{1,4})((?:[ -]?e\d{1,4})*)"
);
re!(EXTRA_EP_RE, r"(?i)([ -]?)e(\d{1,4})");
re!(NXMM_RE, r"(?i)\b(\d{1,2})x(\d{2,3})\b");
re!(
    LONG_RE,
    r"(?i)\bseason[ ]?(\d{1,2})[ ]?episode[ ]?(\d{1,3})\b"
);
re!(DATE_RE, r"\b((?:19|20)\d{2})[ -](\d{2})[ -](\d{2})\b");
re!(BARE_EP_RE, r"(?i)^(?:e|ep|episode)?[ ]?(\d{1,3})\b");
re!(
    SHOW_DIR_CUT_RE,
    r"(?i)\b(?:s\d{1,2}|season[ ]?\d+|complete)\b"
);
re!(
    LANG_RE,
    r"^(?:[a-zA-Z]{2,3}|[a-zA-Z]{2,3}[-_][a-zA-Z]{2,4})$"
);

fn split_path(p: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = p
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let file = parts.pop().unwrap_or("");
    (parts, file)
}

fn file_stem(file: &str) -> &str {
    match file.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => file,
    }
}

/// Classify a relative path. `hint` is the library kind; `None` auto-detects.
pub fn classify(path: &str, hint: Option<LibraryKind>) -> FileClass {
    let (dirs, file) = split_path(path);
    let Some(ext) = extension(file) else {
        return FileClass::Ignored;
    };
    let stem = file_stem(file);
    if dirs.iter().any(|d| is_ignored_dir(d)) {
        return FileClass::Ignored;
    }
    let sl = stem.to_lowercase();
    if sl == "sample" || IGNORED_SUFFIXES.iter().any(|s| sl.ends_with(s)) {
        return FileClass::Ignored;
    }
    if is_video_ext(&ext) {
        match parse_video(&dirs, stem, hint) {
            Some(p) => FileClass::Video(p),
            None => FileClass::Ignored,
        }
    } else if is_subtitle_ext(&ext) {
        FileClass::Subtitle(parse_subtitle(stem, &ext))
    } else {
        FileClass::Ignored
    }
}

/// Parse a video path; `None` if it is not a recognisable video/episode.
pub fn parse_path(path: &str, hint: Option<LibraryKind>) -> Option<ParsedMedia> {
    match classify(path, hint) {
        FileClass::Video(p) => Some(p),
        _ => None,
    }
}

/// Parse a subtitle sidecar stem such as `Movie.en.forced`.
pub fn parse_subtitle(stem: &str, ext: &str) -> SidecarSubtitle {
    let mut base = stem;
    let mut language = None;
    let (mut forced, mut default, mut sdh) = (false, false, false);
    loop {
        let Some((head, tok)) = base.rsplit_once('.') else {
            break;
        };
        if head.is_empty() {
            break;
        }
        let t = tok.to_lowercase();
        match t.as_str() {
            "forced" => forced = true,
            "default" => default = true,
            "sdh" | "hi" | "cc" => sdh = true,
            _ if language.is_none() && LANG_RE.is_match(tok) => {
                language = Some(t.replace('_', "-"));
            }
            _ => break,
        }
        base = head;
    }
    SidecarSubtitle {
        base_stem: base.to_string(),
        full_stem: stem.to_string(),
        language,
        forced,
        default,
        sdh,
        ext: ext.to_string(),
    }
}

// ---------------------------------------------------------------- helpers

struct Tags {
    text: String,
    tmdb: Option<i64>,
    edition: Option<String>,
}

fn strip_tags(s: &str) -> Tags {
    let tmdb = TMDB_RE.captures(s).and_then(|c| c[1].parse::<i64>().ok());
    let edition = EDITION_TAG_RE
        .captures(s)
        .map(|c| c[1].trim().to_string())
        .filter(|e| !e.is_empty());
    let t = TMDB_RE.replace_all(s, "");
    let t = OTHER_ID_RE.replace_all(&t, "");
    let t = EDITION_TAG_RE.replace_all(&t, "");
    Tags {
        text: t.trim().to_string(),
        tmdb,
        edition,
    }
}

fn normalize(s: &str) -> String {
    let spaces = s.matches(' ').count();
    let dots = s.matches('.').count();
    let mut out = s.replace('_', " ");
    if dots > 0 && (spaces == 0 || dots > spaces) {
        out = out.replace('.', " ");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_seps(s: &str) -> &str {
    let s = s.trim_matches([' ', '-', '.', '_', ',']);
    let s = s.trim_end_matches(['(', '[', '{']);
    s.trim_matches([' ', '-', '.', '_', ','])
}

fn clean_title(s: &str) -> String {
    let s = LEADING_GROUP_RE.replace(s, "");
    trim_seps(&s).to_string()
}

fn cut_at_quality(s: &str) -> &str {
    match QUALITY_RE.find(s) {
        Some(m) => &s[..m.start()],
        None => s,
    }
}

/// Returns (title, year, rest-after-year).
fn parse_title_year(s: &str) -> (String, Option<i32>, String) {
    let mut paren: Option<(usize, usize, i32)> = None;
    let mut bare: Option<(usize, usize, i32)> = None;
    for m in YEAR4_RE.find_iter(s) {
        if m.start() == 0 {
            continue;
        }
        let prev = s[..m.start()].chars().next_back();
        let next = s[m.end()..].chars().next();
        let year: i32 = m.as_str().parse().unwrap_or(0);
        let ok_prev = matches!(prev, Some(' ' | '-' | '(' | '['));
        let ok_next = matches!(next, None | Some(' ' | '-' | ')' | ']'));
        if !(ok_prev && ok_next) {
            continue;
        }
        let is_paren = matches!(prev, Some('(' | '[')) && matches!(next, Some(')' | ']'));
        if is_paren {
            // include the brackets in the span
            paren = Some((m.start() - 1, m.end() + 1, year));
        } else if !matches!(prev, Some('(' | '[')) && !matches!(next, Some(')' | ']')) {
            let title = clean_title(&s[..m.start()]);
            if !title.is_empty() {
                bare = Some((m.start(), m.end(), year));
            }
        }
    }
    if let Some((a, b, y)) = paren.or(bare) {
        let title = clean_title(&s[..a]);
        if !title.is_empty() {
            return (title, Some(y), s[b..].to_string());
        }
    }
    (clean_title(cut_at_quality(s)), None, String::new())
}

fn edition_from_rest(rest: &str) -> Option<String> {
    let r = BRACKETS_RE.replace_all(rest, " ");
    let r = cut_at_quality(&r);
    let r = trim_seps(r);
    if r.is_empty() || r.len() > 80 {
        None
    } else {
        Some(r.to_string())
    }
}

fn is_season_dir(d: &str) -> Option<i32> {
    let n = normalize(d);
    if n.eq_ignore_ascii_case("specials") || n.eq_ignore_ascii_case("special") {
        return Some(0);
    }
    SEASON_DIR_RE.captures(&n).and_then(|c| c[1].parse().ok())
}

// ---------------------------------------------------------------- video

fn parse_video(dirs: &[&str], stem: &str, hint: Option<LibraryKind>) -> Option<ParsedMedia> {
    let tags = strip_tags(stem);
    let mut stem_text = normalize(&tags.text);

    // Multi-part: suffix on the file or a `cd1` style parent directory.
    let mut part = None;
    if let Some(c) = PART_SUFFIX_RE.captures(&stem_text) {
        part = c[1].parse().ok();
        let end = c.get(0).unwrap().start();
        stem_text.truncate(end);
    }
    let mut dirs: Vec<&str> = dirs.to_vec();
    if let Some(c) = dirs.last().copied().and_then(|l| PART_DIR_RE.captures(l)) {
        if part.is_none() {
            part = c[1].parse().ok();
        }
        dirs.pop();
    }

    let tv = match hint {
        Some(LibraryKind::Movies) => None,
        Some(LibraryKind::Shows) => parse_episode(&dirs, &stem_text, true),
        None => parse_episode(&dirs, &stem_text, false),
    };
    if let Some(mut ep) = tv {
        ep.tmdb_id = tags.tmdb.or_else(|| dir_tmdb(&dirs));
        ep.part = part;
        ep.edition = tags.edition;
        return Some(ep);
    }
    if hint == Some(LibraryKind::Shows) {
        return None;
    }

    // Movie
    let (mut title, mut year, rest) = parse_title_year(&stem_text);
    let mut edition = tags.edition;
    if edition.is_none() && year.is_some() {
        edition = edition_from_rest(&rest);
    }
    if let (None, Some(dir)) = (year, dirs.last()) {
        let dtags = strip_tags(dir);
        let (dt, dy, _) = parse_title_year(&normalize(&dtags.text));
        if dy.is_some() && !dt.is_empty() {
            title = dt;
            year = dy;
        }
    }
    if title.is_empty() {
        let dir = dirs.last()?;
        let dtags = strip_tags(dir);
        title = parse_title_year(&normalize(&dtags.text)).0;
        if title.is_empty() {
            return None;
        }
    }
    Some(ParsedMedia {
        kind: MediaKind::Movie,
        title,
        year,
        tmdb_id: tags.tmdb.or_else(|| dir_tmdb(&dirs)),
        season: None,
        episodes: vec![],
        air_date: None,
        episode_title: None,
        edition,
        part,
    })
}

fn dir_tmdb(dirs: &[&str]) -> Option<i64> {
    dirs.iter().rev().find_map(|d| strip_tags(d).tmdb)
}

fn parse_episode(dirs: &[&str], stem: &str, hinted: bool) -> Option<ParsedMedia> {
    let season_idx = dirs.iter().rposition(|d| is_season_dir(d).is_some());
    let dir_season = season_idx.and_then(|i| is_season_dir(dirs[i]));

    let mut season: Option<i32> = None;
    let mut episodes: Vec<i32> = vec![];
    let mut air_date = None;
    let (mstart, mend);

    if let Some(c) = SXXEXX_RE.captures(stem) {
        let m = c.get(0).unwrap();
        (mstart, mend) = (m.start(), m.end());
        season = c[1].parse().ok();
        let first: i32 = c[2].parse().ok()?;
        episodes.push(first);
        let mut prev = first;
        for e in EXTRA_EP_RE.captures_iter(&c[3]) {
            let n: i32 = e[2].parse().ok()?;
            if &e[1] == "-" && n > prev && n - prev < 100 {
                episodes.extend(prev + 1..=n);
            } else {
                episodes.push(n);
            }
            prev = n;
        }
    } else if let Some(c) = LONG_RE.captures(stem) {
        let m = c.get(0).unwrap();
        (mstart, mend) = (m.start(), m.end());
        season = c[1].parse().ok();
        episodes.push(c[2].parse().ok()?);
    } else if let Some(c) = NXMM_RE.captures(stem) {
        let m = c.get(0).unwrap();
        (mstart, mend) = (m.start(), m.end());
        season = c[1].parse().ok();
        episodes.push(c[2].parse().ok()?);
    } else if let Some(c) = DATE_RE
        .captures(stem)
        .filter(|c| (hinted || dir_season.is_some()) && valid_date(c))
    {
        let m = c.get(0).unwrap();
        (mstart, mend) = (m.start(), m.end());
        air_date = Some(format!("{}-{}-{}", &c[1], &c[2], &c[3]));
        season = Some(c[1].parse().ok()?);
    } else if let Some(c) = BARE_EP_RE.captures(stem).filter(|_| dir_season.is_some()) {
        let m = c.get(0).unwrap();
        (mstart, mend) = (m.start(), m.end());
        episodes.push(c[1].parse().ok()?);
    } else {
        return None;
    }
    if dir_season.is_some() && (air_date.is_some() || season.is_none()) {
        season = dir_season;
    }
    if season.is_none() {
        season = dir_season;
    }

    // Show title: folder above the season folder (or the nearest folder), else filename.
    let show_dir = match season_idx {
        Some(0) => None,
        Some(i) => Some(dirs[i - 1]),
        None => dirs.last().copied(),
    };
    let mut title = String::new();
    let mut year = None;
    let mut tmdb = None;
    if let Some(d) = show_dir {
        let t = strip_tags(d);
        tmdb = t.tmdb;
        let n = normalize(&t.text);
        let cut = match SHOW_DIR_CUT_RE.find(&n) {
            Some(m) if m.start() > 0 => &n[..m.start()],
            _ => &n[..],
        };
        let (tt, y, _) = parse_title_year(cut);
        title = tt;
        year = y;
    }
    if title.is_empty() {
        let (tt, y, _) = parse_title_year(&stem[..mstart]);
        title = tt;
        year = year.or(y);
    } else if year.is_none() {
        year = parse_title_year(&stem[..mstart]).1;
    }
    if title.is_empty() {
        return None;
    }
    let after = trim_seps(cut_at_quality(&stem[mend..]));
    let after = BRACKETS_RE.replace_all(after, "");
    let after = trim_seps(&after);
    let episode_title = (!after.is_empty()).then(|| after.to_string());

    Some(ParsedMedia {
        kind: MediaKind::Episode,
        title,
        year,
        tmdb_id: tmdb,
        season,
        episodes,
        air_date,
        episode_title,
        edition: None,
        part: None,
    })
}

fn valid_date(c: &regex::Captures) -> bool {
    let m: u32 = c[2].parse().unwrap_or(0);
    let d: u32 = c[3].parse().unwrap_or(0);
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Exp {
        title: &'static str,
        year: Option<i32>,
        tmdb: Option<i64>,
        season: Option<i32>,
        eps: Vec<i32>,
        edition: Option<&'static str>,
        part: Option<i32>,
        air: Option<&'static str>,
        ep_title: Option<&'static str>,
    }

    fn movie(path: &str, e: Exp) {
        let p = parse_path(path, Some(LibraryKind::Movies))
            .unwrap_or_else(|| panic!("no parse: {path}"));
        assert_eq!(p.kind, MediaKind::Movie, "{path}");
        assert_eq!(p.title, e.title, "{path}");
        assert_eq!(p.year, e.year, "{path}");
        assert_eq!(p.tmdb_id, e.tmdb, "{path}");
        assert_eq!(p.edition.as_deref(), e.edition, "{path}");
        assert_eq!(p.part, e.part, "{path}");
    }

    fn ep(path: &str, e: Exp) {
        let p = parse_path(path, Some(LibraryKind::Shows))
            .unwrap_or_else(|| panic!("no parse: {path}"));
        assert_eq!(p.kind, MediaKind::Episode, "{path}");
        assert_eq!(p.title, e.title, "{path}");
        assert_eq!(p.year, e.year, "{path}");
        assert_eq!(p.tmdb_id, e.tmdb, "{path}");
        assert_eq!(p.season, e.season, "{path}");
        assert_eq!(p.episodes, e.eps, "{path}");
        assert_eq!(p.air_date.as_deref(), e.air, "{path}");
        assert_eq!(p.episode_title.as_deref(), e.ep_title, "{path}");
    }

    #[test]
    fn movies_plex_style() {
        movie(
            "The Matrix (1999)/The Matrix (1999).mkv",
            Exp {
                title: "The Matrix",
                year: Some(1999),
                ..Default::default()
            },
        );
        movie(
            "Movies/Inception (2010)/Inception (2010) - Director's Cut.mkv",
            Exp {
                title: "Inception",
                year: Some(2010),
                edition: Some("Director's Cut"),
                ..Default::default()
            },
        );
        movie(
            "Blade Runner (1982)/Blade Runner (1982) {edition-Final Cut}.mp4",
            Exp {
                title: "Blade Runner",
                year: Some(1982),
                edition: Some("Final Cut"),
                ..Default::default()
            },
        );
        movie(
            "Heat (1995)/Heat (1995) - 1080p.mkv",
            Exp {
                title: "Heat",
                year: Some(1995),
                ..Default::default()
            },
        );
        movie(
            "Mr. Robot Movie (2015).mkv",
            Exp {
                title: "Mr. Robot Movie",
                year: Some(2015),
                ..Default::default()
            },
        );
        movie(
            "2012 (2009)/2012 (2009).mkv",
            Exp {
                title: "2012",
                year: Some(2009),
                ..Default::default()
            },
        );
        movie(
            "Blade Runner 2049 (2017).mkv",
            Exp {
                title: "Blade Runner 2049",
                year: Some(2017),
                ..Default::default()
            },
        );
        movie(
            "Amélie (2001).mkv",
            Exp {
                title: "Amélie",
                year: Some(2001),
                ..Default::default()
            },
        );
    }

    #[test]
    fn movies_scene_names() {
        movie(
            "Dune.2021.1080p.BluRay.x264-GRP.mkv",
            Exp {
                title: "Dune",
                year: Some(2021),
                ..Default::default()
            },
        );
        movie(
            "Some.Movie.Name.2019.2160p.UHD.BluRay.REMUX.HDR.HEVC-GRP.mkv",
            Exp {
                title: "Some Movie Name",
                year: Some(2019),
                ..Default::default()
            },
        );
        movie(
            "The.Thing.1982.Extended.Cut.720p.BluRay.x264-GRP.mkv",
            Exp {
                title: "The Thing",
                year: Some(1982),
                edition: Some("Extended Cut"),
                ..Default::default()
            },
        );
        movie(
            "Wonder_Woman_2017_WEB-DL.mkv",
            Exp {
                title: "Wonder Woman",
                year: Some(2017),
                ..Default::default()
            },
        );
        movie(
            "No.Year.Movie.1080p.WEB-DL.DDP5.1.x264-GRP.mkv",
            Exp {
                title: "No Year Movie",
                ..Default::default()
            },
        );
        movie(
            "Title [2010] [1080p].mkv",
            Exp {
                title: "Title",
                year: Some(2010),
                ..Default::default()
            },
        );
        movie(
            "2001.A.Space.Odyssey.1968.1080p.BluRay.mkv",
            Exp {
                title: "2001 A Space Odyssey",
                year: Some(1968),
                ..Default::default()
            },
        );
    }

    #[test]
    fn movies_tags_and_parts() {
        movie(
            "Fight Club (1999) {tmdb-550}/Fight Club (1999) {tmdb-550}.mkv",
            Exp {
                title: "Fight Club",
                year: Some(1999),
                tmdb: Some(550),
                ..Default::default()
            },
        );
        movie(
            "Fight Club (1999) [tmdbid-550].mkv",
            Exp {
                title: "Fight Club",
                year: Some(1999),
                tmdb: Some(550),
                ..Default::default()
            },
        );
        movie(
            "Fight Club (1999) [tmdb-550] [imdbid-tt0137523].mkv",
            Exp {
                title: "Fight Club",
                year: Some(1999),
                tmdb: Some(550),
                ..Default::default()
            },
        );
        movie(
            "Fight Club (1999) {tmdb-550}/Fight Club.mkv",
            Exp {
                title: "Fight Club",
                year: Some(1999),
                tmdb: Some(550),
                ..Default::default()
            },
        );
        movie(
            "Kill Bill (2003)/Kill Bill (2003) - cd1.avi",
            Exp {
                title: "Kill Bill",
                year: Some(2003),
                part: Some(1),
                ..Default::default()
            },
        );
        movie(
            "Kill Bill (2003)/Kill Bill (2003) - part2.avi",
            Exp {
                title: "Kill Bill",
                year: Some(2003),
                part: Some(2),
                ..Default::default()
            },
        );
        movie(
            "Kill Bill (2003)/cd2/movie.avi",
            Exp {
                title: "Kill Bill",
                year: Some(2003),
                part: Some(2),
                ..Default::default()
            },
        );
        movie(
            "Kill.Bill.2003.DVDRip.XviD.CD1.avi",
            Exp {
                title: "Kill Bill",
                year: Some(2003),
                part: Some(1),
                ..Default::default()
            },
        );
        movie(
            "Weird (2000)/Part 1/weird.mkv",
            Exp {
                title: "Weird",
                year: Some(2000),
                part: Some(1),
                ..Default::default()
            },
        );
        movie(
            "Movie Dir (2005)/movie.mkv",
            Exp {
                title: "Movie Dir",
                year: Some(2005),
                ..Default::default()
            },
        );
        movie(
            "Windows Path (2005)\\Windows Path (2005).mkv",
            Exp {
                title: "Windows Path",
                year: Some(2005),
                ..Default::default()
            },
        );
    }

    #[test]
    fn tv_standard() {
        ep(
            "Breaking Bad (2008)/Season 01/Breaking Bad - S01E02 - Cat's in the Bag....mkv",
            Exp {
                title: "Breaking Bad",
                year: Some(2008),
                season: Some(1),
                eps: vec![2],
                ep_title: Some("Cat's in the Bag"),
                ..Default::default()
            },
        );
        ep(
            "The Office (US) (2005)/Season 2/The Office (US) - S02E10.mkv",
            Exp {
                title: "The Office (US)",
                year: Some(2005),
                season: Some(2),
                eps: vec![10],
                ..Default::default()
            },
        );
        ep(
            "Show.Name.S03E04.720p.HDTV.x264-GRP.mkv",
            Exp {
                title: "Show Name",
                season: Some(3),
                eps: vec![4],
                ..Default::default()
            },
        );
        ep(
            "Show Name/Show.Name.s01e05.mkv",
            Exp {
                title: "Show Name",
                season: Some(1),
                eps: vec![5],
                ..Default::default()
            },
        );
        ep(
            "Show {tmdb-1396}/Season 1/Show S01E01.mkv",
            Exp {
                title: "Show",
                tmdb: Some(1396),
                season: Some(1),
                eps: vec![1],
                ..Default::default()
            },
        );
        ep(
            "Show/Season 01/Show - S01E01 [tmdbid-77].mkv",
            Exp {
                title: "Show",
                tmdb: Some(77),
                season: Some(1),
                eps: vec![1],
                ..Default::default()
            },
        );
    }

    #[test]
    fn tv_multi_and_alt_formats() {
        ep(
            "Show/Season 01/Show - S01E02E03.mkv",
            Exp {
                title: "Show",
                season: Some(1),
                eps: vec![2, 3],
                ..Default::default()
            },
        );
        ep(
            "Show/Season 01/Show - S01E02-E04.mkv",
            Exp {
                title: "Show",
                season: Some(1),
                eps: vec![2, 3, 4],
                ..Default::default()
            },
        );
        ep(
            "Show/Season 01/Show - S01E02 - E03.mkv",
            Exp {
                title: "Show",
                season: Some(1),
                eps: vec![2],
                ep_title: Some("E03"),
                ..Default::default()
            },
        );
        ep(
            "Show/Season 1/Show 1x02.mkv",
            Exp {
                title: "Show",
                season: Some(1),
                eps: vec![2],
                ..Default::default()
            },
        );
        ep(
            "Show.12x103.mkv",
            Exp {
                title: "Show",
                season: Some(12),
                eps: vec![103],
                ..Default::default()
            },
        );
        ep(
            "Show Season 2 Episode 5.mkv",
            Exp {
                title: "Show",
                season: Some(2),
                eps: vec![5],
                ..Default::default()
            },
        );
        ep(
            "Show/Season 3/05 - Title Here.mkv",
            Exp {
                title: "Show",
                season: Some(3),
                eps: vec![5],
                ep_title: Some("Title Here"),
                ..Default::default()
            },
        );
        ep(
            "Show/Season 3/Episode 07.mkv",
            Exp {
                title: "Show",
                season: Some(3),
                eps: vec![7],
                ..Default::default()
            },
        );
    }

    #[test]
    fn tv_specials_and_dates() {
        ep(
            "Show/Season 00/Show - S00E01 - Behind.mkv",
            Exp {
                title: "Show",
                season: Some(0),
                eps: vec![1],
                ep_title: Some("Behind"),
                ..Default::default()
            },
        );
        ep(
            "Show/Specials/Show - S00E02.mkv",
            Exp {
                title: "Show",
                season: Some(0),
                eps: vec![2],
                ..Default::default()
            },
        );
        ep(
            "Show/Specials/01 - Pilot.mkv",
            Exp {
                title: "Show",
                season: Some(0),
                eps: vec![1],
                ep_title: Some("Pilot"),
                ..Default::default()
            },
        );
    }

    #[test]
    fn tv_date_based() {
        ep(
            "Daily Show/Season 2024/Daily Show - 2024-01-31 - Guest.mkv",
            Exp {
                title: "Daily Show",
                season: Some(2024),
                air: Some("2024-01-31"),
                ep_title: Some("Guest"),
                ..Default::default()
            },
        );
        ep(
            "Daily Show.2024.02.01.720p.WEB.mkv",
            Exp {
                title: "Daily Show",
                season: Some(2024),
                air: Some("2024-02-01"),
                ..Default::default()
            },
        );
        // invalid month is not a date
        assert!(parse_path("Show 2024-13-45.mkv", Some(LibraryKind::Shows)).is_none());
    }

    #[test]
    fn auto_detect() {
        let p = parse_path("Show - S01E02.mkv", None).unwrap();
        assert_eq!(p.kind, MediaKind::Episode);
        let p = parse_path("Dune.2021.1080p.mkv", None).unwrap();
        assert_eq!(p.kind, MediaKind::Movie);
        // Movie hint ignores episode-like tokens
        let p = parse_path("Show - S01E02.mkv", Some(LibraryKind::Movies)).unwrap();
        assert_eq!(p.kind, MediaKind::Movie);
        // Show library: non-episode file is skipped
        assert!(parse_path("Random Movie (2010).mkv", Some(LibraryKind::Shows)).is_none());
    }

    #[test]
    fn ignores_extras_samples_and_non_video() {
        for p in [
            "Movie (2010)/Sample/sample.mkv",
            "Movie (2010)/Samples/movie-sample.mkv",
            "Movie (2010)/Trailers/trailer.mkv",
            "Movie (2010)/Extras/Making Of.mkv",
            "Movie (2010)/Featurettes/x.mkv",
            "Movie (2010)/Behind The Scenes/x.mkv",
            "Movie (2010)/Movie (2010)-trailer.mkv",
            "Movie.2010.sample.mkv",
            "Movie (2010)/movie.nfo",
            "Movie (2010)/poster.jpg",
            "Movie (2010)/movie.txt",
            "Movie (2010)/noext",
            "Show/Season 1/Extras/S01E01.mkv",
        ] {
            assert_eq!(classify(p, None), FileClass::Ignored, "{p}");
        }
    }

    #[test]
    fn sidecar_subtitles() {
        let FileClass::Subtitle(s) = classify("M (2000)/M (2000).en.forced.srt", None) else {
            panic!()
        };
        assert_eq!(s.base_stem, "M (2000)");
        assert_eq!(s.language.as_deref(), Some("en"));
        assert!(s.forced && !s.sdh);

        let FileClass::Subtitle(s) = classify("movie.pt-BR.srt", None) else {
            panic!()
        };
        assert_eq!(s.base_stem, "movie");
        assert_eq!(s.language.as_deref(), Some("pt-br"));

        let FileClass::Subtitle(s) = classify("movie.eng.sdh.ass", None) else {
            panic!()
        };
        assert_eq!(s.language.as_deref(), Some("eng"));
        assert!(s.sdh);
        assert_eq!(s.ext, "ass");

        let FileClass::Subtitle(s) = classify("movie.srt", None) else {
            panic!()
        };
        assert_eq!(s.base_stem, "movie");
        assert_eq!(s.language, None);

        let s = parse_subtitle("movie.default.en", "srt");
        assert!(s.default);
        assert_eq!(s.language.as_deref(), Some("en"));
    }

    #[test]
    fn helper_predicates() {
        assert!(is_video_ext("mkv"));
        assert!(!is_video_ext("srt"));
        assert!(is_subtitle_ext("srt"));
        assert_eq!(extension("a.MKV").as_deref(), Some("mkv"));
        assert_eq!(extension(".hidden"), None);
        assert!(is_ignored_dir(".git"));
        assert!(is_ignored_dir("Extras"));
        assert!(!is_ignored_dir("Season 01"));
        assert_eq!(LibraryKind::from_db("movies"), Some(LibraryKind::Movies));
        assert_eq!(LibraryKind::from_db("x"), None);
    }
}

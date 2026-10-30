//! Language code normalisation (ISO 639-1 canonical; 639-2/B, 639-2/T, English names and region
//! suffixes accepted) used for audio/subtitle selection.

/// `(639-1, 639-2/B, 639-2/T, name)`
const TABLE: &[(&str, &str, &str, &str)] = &[
    ("en", "eng", "eng", "english"),
    ("ar", "ara", "ara", "arabic"),
    ("nl", "dut", "nld", "dutch"),
    ("fr", "fre", "fra", "french"),
    ("de", "ger", "deu", "german"),
    ("es", "spa", "spa", "spanish"),
    ("it", "ita", "ita", "italian"),
    ("pt", "por", "por", "portuguese"),
    ("ru", "rus", "rus", "russian"),
    ("tr", "tur", "tur", "turkish"),
    ("fa", "per", "fas", "persian"),
    ("he", "heb", "heb", "hebrew"),
    ("hi", "hin", "hin", "hindi"),
    ("ur", "urd", "urd", "urdu"),
    ("ja", "jpn", "jpn", "japanese"),
    ("ko", "kor", "kor", "korean"),
    ("zh", "chi", "zho", "chinese"),
    ("pl", "pol", "pol", "polish"),
    ("sv", "swe", "swe", "swedish"),
    ("da", "dan", "dan", "danish"),
    ("no", "nor", "nor", "norwegian"),
    ("fi", "fin", "fin", "finnish"),
    ("el", "gre", "ell", "greek"),
    ("cs", "cze", "ces", "czech"),
    ("sk", "slo", "slk", "slovak"),
    ("hu", "hun", "hun", "hungarian"),
    ("ro", "rum", "ron", "romanian"),
    ("bg", "bul", "bul", "bulgarian"),
    ("uk", "ukr", "ukr", "ukrainian"),
    ("hr", "hrv", "hrv", "croatian"),
    ("sr", "srp", "srp", "serbian"),
    ("sl", "slv", "slv", "slovenian"),
    ("bs", "bos", "bos", "bosnian"),
    ("th", "tha", "tha", "thai"),
    ("vi", "vie", "vie", "vietnamese"),
    ("id", "ind", "ind", "indonesian"),
    ("ms", "may", "msa", "malay"),
    ("bn", "ben", "ben", "bengali"),
    ("ta", "tam", "tam", "tamil"),
    ("te", "tel", "tel", "telugu"),
    ("ml", "mal", "mal", "malayalam"),
    ("ca", "cat", "cat", "catalan"),
    ("eu", "baq", "eus", "basque"),
    ("gl", "glg", "glg", "galician"),
    ("is", "ice", "isl", "icelandic"),
    ("et", "est", "est", "estonian"),
    ("lv", "lav", "lav", "latvian"),
    ("lt", "lit", "lit", "lithuanian"),
    ("sq", "alb", "sqi", "albanian"),
    ("hy", "arm", "hye", "armenian"),
    ("ka", "geo", "kat", "georgian"),
    ("az", "aze", "aze", "azerbaijani"),
    ("kk", "kaz", "kaz", "kazakh"),
    ("af", "afr", "afr", "afrikaans"),
    ("sw", "swa", "swa", "swahili"),
    ("tl", "tgl", "tgl", "tagalog"),
    ("la", "lat", "lat", "latin"),
    ("ku", "kur", "kur", "kurdish"),
    ("ps", "pus", "pus", "pashto"),
    ("mk", "mac", "mkd", "macedonian"),
    ("mt", "mlt", "mlt", "maltese"),
    ("ga", "gle", "gle", "irish"),
    ("cy", "wel", "cym", "welsh"),
];

/// Variants that fold into a canonical code.
const ALIASES: &[(&str, &str)] = &[
    ("nb", "no"),
    ("nob", "no"),
    ("nn", "no"),
    ("nno", "no"),
    ("cmn", "zh"),
    ("yue", "zh"),
    ("fil", "tl"),
    ("flemish", "nl"),
    ("farsi", "fa"),
    ("iw", "he"),
    ("in", "id"),
    ("deutsch", "de"),
    ("français", "fr"),
    ("francais", "fr"),
    ("español", "es"),
    ("espanol", "es"),
    ("nederlands", "nl"),
    ("italiano", "it"),
    ("português", "pt"),
    ("portugues", "pt"),
    ("русский", "ru"),
    ("العربية", "ar"),
    ("عربي", "ar"),
    ("türkçe", "tr"),
    ("turkce", "tr"),
];

/// Canonical code of a language tag/name: `"eng"`, `"English"`, `"en-US"`, `"ger"` → `"en"`/`"de"`.
/// Unknown three-letter codes are kept (lower-cased); `und`/empty/`zxx` → `None`.
pub fn normalize_lang(raw: &str) -> Option<String> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    if matches!(
        s.as_str(),
        "und" | "unk" | "zxx" | "mis" | "mul" | "undefined" | "unknown" | "none"
    ) {
        return None;
    }
    if let Some((_, c)) = ALIASES.iter().find(|(a, _)| *a == s) {
        return Some((*c).to_string());
    }
    // region / script suffix: en-US, pt_BR, zh-Hans
    let base = s.split(['-', '_']).next().unwrap_or(&s);
    let look = |x: &str| {
        TABLE
            .iter()
            .find(|t| t.0 == x || t.1 == x || t.2 == x || t.3 == x)
            .map(|t| t.0.to_string())
    };
    if let Some(c) = look(&s).or_else(|| look(base)) {
        return Some(c);
    }
    if let Some((_, c)) = ALIASES.iter().find(|(a, _)| *a == base) {
        return Some((*c).to_string());
    }
    (base.len() == 3 && base.chars().all(|c| c.is_ascii_alphabetic())).then(|| base.to_string())
}

/// Do two tags denote the same language?
pub fn same_lang(a: &str, b: &str) -> bool {
    match (normalize_lang(a), normalize_lang(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// English display name of a (normalised) code, for human-readable reasons.
pub fn lang_name(code: &str) -> String {
    TABLE
        .iter()
        .find(|t| t.0 == code)
        .map(|t| {
            let mut c = t.3.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .unwrap_or_else(|| code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codes_names_and_regions() {
        for (raw, want) in [
            ("en", "en"),
            ("eng", "en"),
            ("English", "en"),
            ("en-US", "en"),
            ("EN_gb", "en"),
            ("ar", "ar"),
            ("ara", "ar"),
            ("Arabic", "ar"),
            ("العربية", "ar"),
            ("nl", "nl"),
            ("dut", "nl"),
            ("nld", "nl"),
            ("Dutch", "nl"),
            ("Flemish", "nl"),
            ("fre", "fr"),
            ("fra", "fr"),
            ("ger", "de"),
            ("deu", "de"),
            ("Deutsch", "de"),
            ("pt-BR", "pt"),
            ("por", "pt"),
            ("chi", "zh"),
            ("zho", "zh"),
            ("zh-Hans", "zh"),
            ("cmn", "zh"),
            ("per", "fa"),
            ("fas", "fa"),
            ("Farsi", "fa"),
            ("nb", "no"),
            ("nor", "no"),
            ("jpn", "ja"),
            ("heb", "he"),
            ("iw", "he"),
            ("rus", "ru"),
            ("tur", "tr"),
            ("fil", "tl"),
            ("xyz", "xyz"),
        ] {
            assert_eq!(normalize_lang(raw).as_deref(), Some(want), "{raw}");
        }
        for raw in ["", "  ", "und", "UND", "zxx", "mul", "q", "12345"] {
            assert_eq!(normalize_lang(raw), None, "{raw}");
        }
        assert!(same_lang("eng", "English") && same_lang("dut", "nl") && !same_lang("en", "de"));
        assert!(!same_lang("und", "und"));
        assert_eq!(lang_name("ar"), "Arabic");
        assert_eq!(lang_name("xyz"), "xyz");
    }
}

//! Automatic audio/subtitle selection from per-user preferences.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    decision::is_image_subtitle,
    langs::{lang_name, normalize_lang},
    model::{StreamInfo, StreamKind},
    profile::DeviceProfile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleMode {
    /// Never enable subtitles automatically.
    Off,
    /// Only forced tracks (foreign-language parts) in the audio language.
    ForcedOnly,
    /// Enable when the audio is a language you don't understand, or a forced track exists.
    Auto,
    /// Always the best track in your subtitle languages.
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct PlaybackPrefs {
    /// Ordered audio languages (ISO 639-1/2 or names); `"original"` = the film's original language.
    pub audio_languages: Vec<String>,
    pub subtitle_mode: SubtitleMode,
    /// Ordered subtitle languages.
    pub subtitle_languages: Vec<String>,
    /// Prefer SDH / hearing-impaired tracks.
    pub prefer_sdh: bool,
    /// Allow picking image subtitles (PGS/VobSub) that must be burned into the video.
    pub burn_in_image_subs: bool,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            audio_languages: vec!["original".into()],
            subtitle_mode: SubtitleMode::Auto,
            subtitle_languages: vec!["en".into()],
            prefer_sdh: false,
            burn_in_image_subs: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Selected {
    /// ffprobe index of the chosen audio stream.
    pub audio_index: Option<i64>,
    /// ffprobe index of the chosen subtitle stream; `null` = none.
    pub subtitle_index: Option<i64>,
    /// Why (human readable).
    pub reason: String,
}

/// What the caller (client) asked for explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Not specified: choose automatically.
    Auto,
    /// Subtitles explicitly off (`subtitle_index = -1`).
    Off,
    /// An explicit stream index.
    Index(i64),
}

impl Pick {
    /// `None` → auto, `-1` → off (subtitles), `n` → that stream.
    pub fn from_request(v: Option<i64>) -> Pick {
        match v {
            None => Pick::Auto,
            Some(n) if n < 0 => Pick::Off,
            Some(n) => Pick::Index(n),
        }
    }
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// SDH / hearing-impaired from the track title (`.sdh.`, `.hi.`, `.cc.`, "hearing impaired").
pub fn is_sdh(s: &StreamInfo) -> bool {
    let Some(t) = s.title.as_deref() else {
        return false;
    };
    let toks = tokens(t);
    toks.iter()
        .any(|x| matches!(x.as_str(), "sdh" | "cc" | "hearing" | "hoh"))
        || toks.windows(2).any(|w| w[0] == "hi" && w[1] != "res")
        || toks.last().is_some_and(|x| x == "hi")
}

fn norm_list(v: &[String]) -> Vec<String> {
    v.iter()
        .filter_map(|l| {
            if l.trim().eq_ignore_ascii_case("original") {
                Some("original".to_string())
            } else {
                normalize_lang(l)
            }
        })
        .collect()
}

fn lang_of(s: &StreamInfo) -> Option<String> {
    s.language.as_deref().and_then(normalize_lang)
}

fn default_audio(audio: &[&StreamInfo]) -> Option<i64> {
    audio
        .iter()
        .find(|a| a.default)
        .or_else(|| audio.first())
        .map(|a| a.idx)
}

/// Choose audio + subtitles.
///
/// `original_language` is the title's original language (TMDB) when known.
pub fn select(
    streams: &[StreamInfo],
    prefs: &PlaybackPrefs,
    original_language: Option<&str>,
    audio_req: Pick,
    sub_req: Pick,
    profile: &DeviceProfile,
) -> Selected {
    let audio: Vec<&StreamInfo> = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .collect();
    let subs: Vec<&StreamInfo> = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .collect();
    let mut why: Vec<String> = vec![];

    // ── audio ──
    let original = original_language.and_then(normalize_lang);
    let audio_idx = match audio_req {
        Pick::Index(i) => {
            why.push("audio chosen by the client".into());
            Some(i)
        }
        _ => {
            let mut found = None;
            for pref in norm_list(&prefs.audio_languages) {
                let want = if pref == "original" {
                    original.clone()
                } else {
                    Some(pref.clone())
                };
                let Some(want) = want else { continue };
                // best track in that language: default flag first, then more channels
                let best = audio
                    .iter()
                    .filter(|a| lang_of(a).as_deref() == Some(want.as_str()))
                    .max_by_key(|a| (a.default, a.channels.unwrap_or(0)));
                if let Some(b) = best {
                    why.push(if pref == "original" {
                        format!("audio: original language ({})", lang_name(&want))
                    } else {
                        format!("audio: preferred language {}", lang_name(&want))
                    });
                    found = Some(b.idx);
                    break;
                }
            }
            if found.is_none() {
                found = default_audio(&audio);
                why.push("audio: the file's default track".into());
            }
            found
        }
    };
    let audio_lang = audio_idx
        .and_then(|i| audio.iter().find(|a| a.idx == i))
        .and_then(|a| lang_of(a))
        // single untagged audio track of a film whose original language we know
        .or_else(|| {
            (audio.len() == 1 && audio[0].language.is_none())
                .then(|| original.clone())
                .flatten()
        });

    // ── subtitles ──
    let sub_idx = match sub_req {
        Pick::Index(i) => {
            why.push("subtitle chosen by the client".into());
            Some(i)
        }
        Pick::Off => {
            why.push("subtitles turned off by the client".into());
            None
        }
        Pick::Auto => pick_subtitle(&subs, prefs, audio_lang.as_deref(), profile, &mut why),
    };
    Selected {
        audio_index: audio_idx,
        subtitle_index: sub_idx,
        reason: why.join("; "),
    }
}

fn pick_subtitle(
    subs: &[&StreamInfo],
    prefs: &PlaybackPrefs,
    audio_lang: Option<&str>,
    profile: &DeviceProfile,
    why: &mut Vec<String>,
) -> Option<i64> {
    if subs.is_empty() {
        why.push("subtitles: none available".into());
        return None;
    }
    let sub_langs: Vec<String> = norm_list(&prefs.subtitle_languages)
        .into_iter()
        .filter(|l| l != "original")
        .collect();
    let aud_langs: Vec<String> = norm_list(&prefs.audio_languages);
    // image subtitles need native support or permission to burn them in
    let image_ok = prefs.burn_in_image_subs || !profile.subtitles.image.is_empty();
    let usable: Vec<&StreamInfo> = subs
        .iter()
        .copied()
        .filter(|s| image_ok || !is_image_subtitle(s.codec.as_deref().unwrap_or("")))
        .collect();

    // best track of a language: full before forced (unless `forced` asked), SDH per preference,
    // text before image, default flag, embedded before external
    let best = |lang: &str, forced: bool| -> Option<&StreamInfo> {
        usable
            .iter()
            .copied()
            .filter(|s| lang_of(s).as_deref() == Some(lang) && s.forced == forced)
            .max_by_key(|s| {
                (
                    is_sdh(s) == prefs.prefer_sdh,
                    !is_image_subtitle(s.codec.as_deref().unwrap_or("")),
                    s.default,
                    s.external_path.is_none(),
                    std::cmp::Reverse(s.idx),
                )
            })
    };
    let describe = |s: &StreamInfo| {
        format!(
            "{}{}{}",
            lang_of(s)
                .map(|l| lang_name(&l))
                .unwrap_or_else(|| "unknown".into()),
            if s.forced { " forced" } else { "" },
            if is_sdh(s) { " SDH" } else { "" }
        )
    };
    let full_in_prefs = |why: &mut Vec<String>| -> Option<i64> {
        for l in &sub_langs {
            if let Some(s) = best(l, false) {
                why.push(format!("subtitles: {}", describe(s)));
                return Some(s.idx);
            }
        }
        None
    };
    let forced_in_audio = |why: &mut Vec<String>| -> Option<i64> {
        let l = audio_lang?;
        best(l, true).map(|s| {
            why.push(format!(
                "subtitles: {} track for the audio language",
                describe(s)
            ));
            s.idx
        })
    };

    match prefs.subtitle_mode {
        SubtitleMode::Off => {
            why.push("subtitles: off".into());
            None
        }
        SubtitleMode::ForcedOnly => {
            let r = forced_in_audio(why);
            if r.is_none() {
                why.push("subtitles: no forced track for the audio language".into());
            }
            r
        }
        SubtitleMode::Always => {
            let r = full_in_prefs(why);
            if r.is_none() {
                why.push("subtitles: none in your languages".into());
            }
            r
        }
        SubtitleMode::Auto => {
            let understood = audio_lang
                .is_some_and(|l| aud_langs.iter().chain(sub_langs.iter()).any(|x| x == l));
            match audio_lang {
                Some(l) if !understood => {
                    why.push(format!(
                        "audio is {} (not one of your languages)",
                        lang_name(l)
                    ));
                    let r = full_in_prefs(why);
                    if r.is_none() {
                        why.push("subtitles: none in your languages".into());
                    }
                    r
                }
                _ => {
                    let r = forced_in_audio(why);
                    if r.is_none() {
                        why.push("subtitles: not needed".into());
                    }
                    r
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aud(idx: i64, lang: &str, default: bool, ch: i64) -> StreamInfo {
        let mut s = StreamInfo::new(idx, StreamKind::Audio, "aac");
        s.language = (!lang.is_empty()).then(|| lang.to_string());
        s.default = default;
        s.channels = Some(ch);
        s
    }
    fn sub(idx: i64, lang: &str, forced: bool, title: &str, codec: &str) -> StreamInfo {
        let mut s = StreamInfo::new(idx, StreamKind::Subtitle, codec);
        s.language = (!lang.is_empty()).then(|| lang.to_string());
        s.forced = forced;
        s.title = (!title.is_empty()).then(|| title.to_string());
        s
    }
    fn chrome() -> DeviceProfile {
        DeviceProfile::preset("web_chrome").unwrap()
    }
    fn webos() -> DeviceProfile {
        DeviceProfile::preset("webos").unwrap()
    }
    fn prefs(mode: SubtitleMode, audio: &[&str], subs: &[&str]) -> PlaybackPrefs {
        PlaybackPrefs {
            audio_languages: audio.iter().map(|s| s.to_string()).collect(),
            subtitle_mode: mode,
            subtitle_languages: subs.iter().map(|s| s.to_string()).collect(),
            prefer_sdh: false,
            burn_in_image_subs: false,
        }
    }
    fn sel(streams: &[StreamInfo], p: &PlaybackPrefs, orig: Option<&str>) -> Selected {
        select(streams, p, orig, Pick::Auto, Pick::Auto, &chrome())
    }

    #[test]
    fn sdh_detection() {
        for (t, want) in [
            ("Movie.en.sdh.srt", true),
            ("English SDH", true),
            ("Movie.en.hi.srt", true),
            ("CC", true),
            ("English [HI]", true),
            ("Hearing Impaired", true),
            ("English", false),
            ("Hi-Res", false),
            ("Chi", false),
        ] {
            assert_eq!(is_sdh(&sub(0, "en", false, t, "srt")), want, "{t}");
        }
    }

    #[test]
    fn audio_selection_matrix() {
        let streams = [
            aud(1, "jpn", true, 2),
            aud(2, "eng", false, 6),
            aud(3, "eng", false, 2),
        ];
        // preferred language beats the file default; more channels win among equals
        assert_eq!(
            sel(&streams, &prefs(SubtitleMode::Off, &["en"], &["en"]), None).audio_index,
            Some(2)
        );
        // `original` = the TMDB original language when known …
        assert_eq!(
            sel(
                &streams,
                &prefs(SubtitleMode::Off, &["original"], &[]),
                Some("ja")
            )
            .audio_index,
            Some(1)
        );
        // … else the file's default track
        assert_eq!(
            sel(
                &streams,
                &prefs(SubtitleMode::Off, &["original"], &[]),
                None
            )
            .audio_index,
            Some(1)
        );
        // first available preference wins
        assert_eq!(
            sel(
                &streams,
                &prefs(SubtitleMode::Off, &["nl", "English"], &[]),
                None
            )
            .audio_index,
            Some(2)
        );
        // nothing matches → default track
        let r = sel(&streams, &prefs(SubtitleMode::Off, &["ar"], &[]), None);
        assert_eq!(r.audio_index, Some(1));
        assert!(r.reason.contains("default track"), "{}", r.reason);
        // client index wins
        let r = select(
            &streams,
            &PlaybackPrefs::default(),
            None,
            Pick::Index(3),
            Pick::Auto,
            &chrome(),
        );
        assert_eq!(r.audio_index, Some(3));
        // codes: ISO 639-2 tags and region suffixes
        let s2 = [
            aud(1, "ger", true, 2),
            aud(2, "ara", false, 2),
            aud(3, "en-US", false, 2),
        ];
        assert_eq!(
            sel(&s2, &prefs(SubtitleMode::Off, &["ar"], &[]), None).audio_index,
            Some(2)
        );
        assert_eq!(
            sel(&s2, &prefs(SubtitleMode::Off, &["eng"], &[]), None).audio_index,
            Some(3)
        );
        // no audio at all
        assert_eq!(sel(&[], &PlaybackPrefs::default(), None).audio_index, None);
    }

    #[test]
    fn subtitle_modes() {
        let streams = [
            aud(1, "ara", true, 2),
            sub(2, "eng", false, "English", "subrip"),
            sub(3, "eng", false, "English SDH", "subrip"),
            sub(4, "eng", true, "Forced", "subrip"),
            sub(5, "ara", true, "Forced", "subrip"),
            sub(6, "eng", false, "English PGS", "hdmv_pgs_subtitle"),
            sub(7, "dut", false, "Nederlands", "subrip"),
        ];
        let p = |m| prefs(m, &["en"], &["en"]);
        // auto: Arabic audio, user does not understand it → full English track (text, non-SDH)
        let r = sel(&streams, &p(SubtitleMode::Auto), None);
        assert_eq!(r.subtitle_index, Some(2), "{}", r.reason);
        assert!(
            r.reason.contains("Arabic") && r.reason.contains("English"),
            "{}",
            r.reason
        );
        // prefer_sdh flips the choice
        let mut sdh = p(SubtitleMode::Auto);
        sdh.prefer_sdh = true;
        assert_eq!(sel(&streams, &sdh, None).subtitle_index, Some(3));
        // always behaves like auto for a foreign language, and also for an understood one
        assert_eq!(
            sel(&streams, &p(SubtitleMode::Always), None).subtitle_index,
            Some(2)
        );
        let eng_audio = [
            aud(1, "eng", true, 2),
            sub(2, "eng", false, "English", "subrip"),
            sub(4, "eng", true, "Forced", "subrip"),
        ];
        assert_eq!(
            sel(&eng_audio, &p(SubtitleMode::Always), None).subtitle_index,
            Some(2),
            "full before forced"
        );
        // auto + understood audio: only the forced track of the audio language
        assert_eq!(
            sel(&eng_audio, &p(SubtitleMode::Auto), None).subtitle_index,
            Some(4)
        );
        assert_eq!(
            sel(&eng_audio[..2], &p(SubtitleMode::Auto), None).subtitle_index,
            None
        );
        // forced_only: forced track in the *audio* language only
        assert_eq!(
            sel(&streams, &p(SubtitleMode::ForcedOnly), None).subtitle_index,
            Some(5)
        );
        assert_eq!(
            sel(&eng_audio[..2], &p(SubtitleMode::ForcedOnly), None).subtitle_index,
            None
        );
        // off
        assert_eq!(
            sel(&streams, &p(SubtitleMode::Off), None).subtitle_index,
            None
        );
        // understood language list includes the audio languages: Dutch audio with nl in audio prefs
        let nl = [
            aud(1, "dut", true, 2),
            sub(2, "eng", false, "English", "subrip"),
        ];
        assert_eq!(
            sel(
                &nl,
                &prefs(SubtitleMode::Auto, &["nl", "en"], &["en"]),
                None
            )
            .subtitle_index,
            None
        );
        assert_eq!(
            sel(&nl, &prefs(SubtitleMode::Auto, &["en"], &["en"]), None).subtitle_index,
            Some(2)
        );
        // untagged audio of a film whose original language is foreign
        let untagged = [
            aud(1, "", true, 2),
            sub(2, "eng", false, "English", "subrip"),
        ];
        assert_eq!(
            sel(&untagged, &p(SubtitleMode::Auto), Some("fr")).subtitle_index,
            Some(2)
        );
        assert_eq!(
            sel(&untagged, &p(SubtitleMode::Auto), None).subtitle_index,
            None,
            "unknown language: do not guess"
        );
        // multiple subtitle languages: first preference with a track
        let multi = [
            aud(1, "jpn", true, 2),
            sub(2, "nld", false, "", "subrip"),
            sub(3, "ara", false, "", "subrip"),
        ];
        assert_eq!(
            sel(
                &multi,
                &prefs(SubtitleMode::Auto, &["en"], &["ar", "nl"]),
                None
            )
            .subtitle_index,
            Some(3)
        );
        assert_eq!(
            sel(&multi, &prefs(SubtitleMode::Auto, &["en"], &["fr"]), None).subtitle_index,
            None
        );
    }

    #[test]
    fn image_subtitles_need_permission_or_native_support() {
        let streams = [
            aud(1, "jpn", true, 2),
            sub(2, "eng", false, "PGS", "hdmv_pgs_subtitle"),
        ];
        let p = prefs(SubtitleMode::Auto, &["en"], &["en"]);
        // web: image subs would need burn-in (a transcode): not chosen by default
        assert_eq!(
            select(&streams, &p, None, Pick::Auto, Pick::Auto, &chrome()).subtitle_index,
            None
        );
        let mut burn = p.clone();
        burn.burn_in_image_subs = true;
        assert_eq!(
            select(&streams, &burn, None, Pick::Auto, Pick::Auto, &chrome()).subtitle_index,
            Some(2)
        );
        // a client that renders PGS natively does not need the permission (if the preset says so)
        let mut native = webos();
        native.subtitles.image = vec!["pgs".into()];
        assert_eq!(
            select(&streams, &p, None, Pick::Auto, Pick::Auto, &native).subtitle_index,
            Some(2)
        );
        // text is preferred over image even when both exist
        let both = [
            aud(1, "jpn", true, 2),
            sub(2, "eng", false, "PGS", "hdmv_pgs_subtitle"),
            sub(3, "eng", false, "SRT", "subrip"),
        ];
        assert_eq!(
            select(&both, &burn, None, Pick::Auto, Pick::Auto, &chrome()).subtitle_index,
            Some(3)
        );
    }

    #[test]
    fn client_overrides_and_sidecars() {
        let mut ext = sub(9, "en", false, "Movie.en.sdh.srt", "srt");
        ext.external_path = Some("/m/Movie.en.sdh.srt".into());
        let streams = [
            aud(1, "jpn", true, 2),
            sub(2, "eng", false, "English", "subrip"),
            ext,
        ];
        let p = prefs(SubtitleMode::Auto, &["en"], &["en"]);
        // explicit index / explicit off override the preferences
        assert_eq!(
            select(&streams, &p, None, Pick::Auto, Pick::Index(9), &chrome()).subtitle_index,
            Some(9)
        );
        assert_eq!(
            select(&streams, &p, None, Pick::Auto, Pick::Off, &chrome()).subtitle_index,
            None
        );
        // sidecar tokens: `.en.sdh.` is SDH
        let mut sdh = p.clone();
        sdh.prefer_sdh = true;
        assert_eq!(
            select(&streams, &sdh, None, Pick::Auto, Pick::Auto, &chrome()).subtitle_index,
            Some(9)
        );
        assert_eq!(Pick::from_request(None), Pick::Auto);
        assert_eq!(Pick::from_request(Some(-1)), Pick::Off);
        assert_eq!(Pick::from_request(Some(4)), Pick::Index(4));
    }
}

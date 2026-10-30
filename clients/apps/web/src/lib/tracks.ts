import { iso1, isSdh, languageName, type AudioStreamInfo, type SubtitleStreamInfo } from "@reel/ui";
import type { PlaybackPrefs } from "../api/prefs";

/**
 * Track choice. The server picks audio + subtitles from the user's preferences
 * (`selected` on decide / vod play); this module:
 *  - previews that choice on the item page before any session exists (same rules as
 *    `server/crates/playback/src/select.rs`, minus TMDB's original language), and
 *  - turns a manual change in the player into a one-tap "save as my preference" offer.
 */

type Aud = Pick<AudioStreamInfo, "index" | "language" | "channels"> & { default?: boolean; title?: string | null };
type Sub = Pick<SubtitleStreamInfo, "index" | "language" | "forced"> & { title?: string | null; codec?: string | null; external?: boolean };

export interface Preview {
  audio: number | null;
  /** `null` = off */
  sub: number | null;
}

const isImage = (codec?: string | null) => /pgs|dvd_sub|dvb_sub|vobsub|hdmv/i.test(codec ?? "");

/** Language of a subtitle, falling back to the file name ("Movie.en.hi.srt"). */
export function subLanguage(s: { language?: string | null; title?: string | null }): string {
  const l = iso1(s.language);
  if (l) return l;
  const m = /[._-]([a-z]{2,3})(?:[._-](?:hi|sdh|cc|forced))*\.(?:srt|ass|ssa|vtt|sub|sup)$/i.exec(s.title ?? "");
  return m ? iso1(m[1]) : "";
}

/** Client-side estimate of the server's automatic choice (item page). */
export function previewSelection(audio: Aud[], subs: Sub[], p: PlaybackPrefs, imageSubsNative = false): Preview {
  const def = audio.find((a) => a.default) ?? audio[0];
  // Without TMDB here, "original" ≈ the file's default track (true for almost every release).
  const original = def ? iso1(def.language) : "";
  let pickedAudio: Aud | undefined;
  for (const pref of p.audio_languages) {
    const want = pref === "original" ? original : iso1(pref);
    if (!want) continue;
    const cands = audio.filter((a) => iso1(a.language) === want);
    if (cands.length) {
      pickedAudio = cands.sort((a, b) => Number(!!b.default) - Number(!!a.default) || (b.channels ?? 0) - (a.channels ?? 0))[0];
      break;
    }
  }
  pickedAudio = pickedAudio ?? def;
  const audioLang = pickedAudio ? iso1(pickedAudio.language) || (audio.length === 1 ? original : "") : "";

  const subLangs = p.subtitle_languages.map(iso1).filter(Boolean);
  // Like the server: "original" isn't a language you know — only the ones you listed.
  const audLangs = p.audio_languages.filter((l) => l !== "original").map(iso1).filter(Boolean);
  const imageOk = p.burn_in_image_subs || imageSubsNative;
  const usable = subs.filter((s) => imageOk || !isImage(s.codec));
  const best = (lang: string, forced: boolean): Sub | undefined =>
    usable
      .filter((s) => subLanguage(s) === lang && !!s.forced === forced)
      .sort(
        (a, b) =>
          Number(isSdh(b) === p.prefer_sdh) - Number(isSdh(a) === p.prefer_sdh) ||
          Number(!isImage(b.codec)) - Number(!isImage(a.codec)) ||
          Number(!b.external) - Number(!a.external) ||
          a.index - b.index,
      )[0];
  const fullInPrefs = () => {
    for (const l of subLangs) {
      const s = best(l, false);
      if (s) return s.index;
    }
    return null;
  };
  const forcedInAudio = () => (audioLang ? best(audioLang, true)?.index ?? null : null);

  let sub: number | null = null;
  if (subs.length) {
    switch (p.subtitle_mode) {
      case "off":
        sub = null;
        break;
      case "forced_only":
        sub = forcedInAudio();
        break;
      case "always":
        sub = fullInPrefs();
        break;
      case "auto": {
        const understood = !!audioLang && (audLangs.indexOf(audioLang) >= 0 || subLangs.indexOf(audioLang) >= 0);
        sub = audioLang && !understood ? fullInPrefs() : forcedInAudio();
        break;
      }
    }
  }
  return { audio: pickedAudio?.index ?? null, sub };
}

/** A one-tap "make this my default" offer after a manual track change. */
export interface QuickSave {
  /** Question shown in the player, e.g. "Always use English subtitles?" */
  text: string;
  patch: Partial<PlaybackPrefs>;
}

const front = (list: string[], code: string) => [code].concat(list.filter((l) => l !== code));

/**
 * What to offer after the viewer picked subtitle `s` (or turned them off) by hand.
 * `audioIsDefault`: the playing audio is the file's default track (≈ the original language).
 */
export function subtitleQuickSave(s: Sub | null, p: PlaybackPrefs, audioLang: string, audioIsDefault: boolean): QuickSave | undefined {
  // Same rule as the server: only listed languages count as understood ("original" doesn't).
  void audioIsDefault;
  const known = p.audio_languages
    .concat(p.subtitle_languages)
    .filter((l) => l !== "original")
    .map(iso1)
    .filter(Boolean);
  const understood = !!audioLang && known.indexOf(audioLang) >= 0;
  // Would the current settings turn (full) subtitles on for this audio?
  const wouldShow = p.subtitle_mode === "always" || (p.subtitle_mode === "auto" && !!audioLang && !understood);
  if (!s) {
    if (!wouldShow) return undefined;
    if (p.subtitle_mode === "auto") {
      // Auto turned them on because of the audio language: learn that it's understood.
      return {
        text: `Skip subtitles for ${languageName(audioLang)} audio from now on?`,
        patch: { audio_languages: p.audio_languages.concat(audioLang) },
      };
    }
    return { text: "Only turn subtitles on when the audio is in a language you don't know?", patch: { subtitle_mode: "auto" } };
  }
  if (s.forced) return undefined;
  const lang = subLanguage(s);
  if (!lang) return undefined;
  const sdh = isSdh(s);
  const name = `${languageName(lang)}${sdh ? " SDH" : ""}`;
  const firstLang = iso1(p.subtitle_languages[0]);
  if (wouldShow && firstLang === lang && p.prefer_sdh === sdh) return undefined;
  const patch: Partial<PlaybackPrefs> = { subtitle_languages: front(p.subtitle_languages.map(iso1), lang) };
  if (sdh !== p.prefer_sdh) patch.prefer_sdh = sdh;
  if (!wouldShow) {
    patch.subtitle_mode = "always";
    return { text: `Always use ${name} subtitles?`, patch };
  }
  return { text: `Prefer ${name} subtitles from now on?`, patch };
}

/** What to offer after the viewer picked audio track `a` by hand. */
export function audioQuickSave(a: Aud | undefined, p: PlaybackPrefs, all: Aud[]): QuickSave | undefined {
  if (!a) return undefined;
  const lang = iso1(a.language);
  if (!lang) return undefined;
  const first = p.audio_languages[0];
  const def = all.find((x) => x.default) ?? all[0];
  if (first === "original" && def && iso1(def.language) === lang) return undefined;
  if (iso1(first) === lang) return undefined;
  return { text: `Always prefer ${languageName(lang)} audio?`, patch: { audio_languages: front(p.audio_languages, lang) } };
}

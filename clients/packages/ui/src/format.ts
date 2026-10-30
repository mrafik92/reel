import type { AudioStreamInfo, Hdr, ItemSummary, MediaFile, SubtitleStreamInfo, VideoStreamInfo } from "./types";

let apiBase = "";
/** Base origin for relative API URLs (empty = same origin). Needed when packaged on a TV (file://). */
export function setApiBase(base: string): void {
  apiBase = base.replace(/\/$/, "");
}

/** Resolve a server-relative URL and (optionally) request a specific image width. */
export function mediaUrl(url: string | null | undefined, width?: number): string | undefined {
  if (!url) return undefined;
  if (/^(data|blob):/.test(url)) return url;
  let u = url;
  if (width) {
    u = /[?&]w=\d+/.test(u) ? u.replace(/([?&]w=)\d+/, `$1${width}`) : `${u}${u.indexOf("?") >= 0 ? "&" : "?"}w=${width}`;
  }
  return u.charAt(0) === "/" ? apiBase + u : u;
}

export function formatRuntime(seconds: number | null | undefined): string {
  if (!seconds) return "";
  const m = Math.round(seconds / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h}h ${r}m` : `${h}h`;
}

/** 1:02:03 / 4:05 */
export function formatClock(seconds: number): string {
  if (!isFinite(seconds) || seconds < 0) seconds = 0;
  const s = Math.floor(seconds % 60);
  const m = Math.floor((seconds / 60) % 60);
  const h = Math.floor(seconds / 3600);
  const pad = (n: number) => (n < 10 ? "0" + n : String(n));
  return h ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function formatRemaining(durationS: number, progress: number): string {
  const left = Math.max(0, durationS * (1 - progress));
  const m = Math.round(left / 60);
  if (m < 1) return "Almost done";
  if (m < 60) return `${m} min left`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m left`;
}

export function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let n = bytes;
  while (n >= 1000 && i < u.length - 1) {
    n /= 1000;
    i++;
  }
  return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
}

export function formatSpeed(bps: number): string {
  return formatBytes(bps) + "/s";
}

export function formatEta(seconds: number): string {
  if (seconds < 60) return "< 1 min left";
  const m = Math.round(seconds / 60);
  if (m < 60) return `${m} min left`;
  return `${Math.floor(m / 60)}h ${m % 60}m left`;
}

export function resolutionLabel(v: VideoStreamInfo | null | undefined): string {
  if (!v) return "";
  if (v.width >= 3200 || v.height >= 2000) return "4K";
  if (v.width >= 1800 || v.height >= 1000) return "1080p";
  if (v.width >= 1200 || v.height >= 700) return "720p";
  return "SD";
}

export function hdrLabel(h: Hdr): string {
  return h === "dv" ? "Dolby Vision" : h === "hdr10" ? "HDR10" : h === "hlg" ? "HLG" : "";
}

const CODEC_NAMES: Record<string, string> = {
  h264: "H.264", avc: "H.264", hevc: "HEVC", h265: "HEVC", av1: "AV1", vp9: "VP9", mpeg2video: "MPEG-2",
  aac: "AAC", ac3: "Dolby Digital", eac3: "Dolby Digital+", truehd: "TrueHD", dts: "DTS", "dts-hd": "DTS-HD",
  flac: "FLAC", opus: "Opus", mp3: "MP3",
  subrip: "SRT", srt: "SRT", ass: "ASS", webvtt: "WebVTT", mov_text: "Text",
  hdmv_pgs_subtitle: "PGS", pgs: "PGS", dvd_subtitle: "VobSub",
};
export function codecName(c: string): string {
  return CODEC_NAMES[c.toLowerCase()] ?? c.toUpperCase();
}

export function channelsLabel(n: number): string {
  if (n === 1) return "Mono";
  if (n === 2) return "Stereo";
  if (n === 6) return "5.1";
  if (n === 8) return "7.1";
  return `${n}ch`;
}

const LANGS: Record<string, string> = {
  en: "English", eng: "English", fr: "French", fre: "French", fra: "French", de: "German", ger: "German",
  deu: "German", es: "Spanish", spa: "Spanish", it: "Italian", ita: "Italian", ja: "Japanese", jpn: "Japanese",
  ko: "Korean", kor: "Korean", pt: "Portuguese", por: "Portuguese", nl: "Dutch", dut: "Dutch", nld: "Dutch",
  sv: "Swedish", swe: "Swedish", da: "Danish", dan: "Danish", no: "Norwegian", nor: "Norwegian", nb: "Norwegian",
  pl: "Polish", pol: "Polish", ru: "Russian", rus: "Russian", zh: "Chinese", chi: "Chinese", zho: "Chinese",
  ar: "Arabic", ara: "Arabic", hi: "Hindi", hin: "Hindi", tr: "Turkish", tur: "Turkish", fi: "Finnish", fin: "Finnish",
  el: "Greek", gre: "Greek", ell: "Greek", he: "Hebrew", heb: "Hebrew", cs: "Czech", cze: "Czech", ces: "Czech",
  hu: "Hungarian", hun: "Hungarian", ro: "Romanian", rum: "Romanian", ron: "Romanian", th: "Thai", tha: "Thai",
  vi: "Vietnamese", vie: "Vietnamese", id: "Indonesian", ind: "Indonesian", uk: "Ukrainian", ukr: "Ukrainian",
  fa: "Persian", per: "Persian", fas: "Persian", bg: "Bulgarian", bul: "Bulgarian", hr: "Croatian", hrv: "Croatian",
  sr: "Serbian", srp: "Serbian", sk: "Slovak", slo: "Slovak", slk: "Slovak", sl: "Slovenian", slv: "Slovenian",
  ms: "Malay", may: "Malay", msa: "Malay", is: "Icelandic", ice: "Icelandic", isl: "Icelandic",
};
/** ISO 639-2/B → 639-2/T (Intl only knows the T forms). */
const B2T: Record<string, string> = {
  fre: "fra", ger: "deu", dut: "nld", chi: "zho", cze: "ces", gre: "ell", per: "fas", rum: "ron", slo: "slk",
  wel: "cym", arm: "hye", baq: "eus", bur: "mya", geo: "kat", ice: "isl", mac: "mkd", mao: "mri", may: "msa",
  alb: "sqi", tib: "bod",
};
let displayNames: { of(code: string): string | undefined } | null | undefined;
function intlName(code: string): string | undefined {
  if (displayNames === undefined) {
    try {
      // Chrome 81+; older TV browsers fall back to the table above.
      const DN = (Intl as unknown as { DisplayNames?: new (l: string[], o: { type: string }) => { of(c: string): string | undefined } }).DisplayNames;
      displayNames = DN ? new DN(["en"], { type: "language" }) : null;
    } catch {
      displayNames = null;
    }
  }
  if (!displayNames) return undefined;
  try {
    const n = displayNames.of(code);
    return n && n.toLowerCase() !== code.toLowerCase() ? n.charAt(0).toUpperCase() + n.slice(1) : undefined;
  } catch {
    return undefined;
  }
}

/** True when a language code is missing or explicitly undetermined. */
export function isUnknownLanguage(code: string | null | undefined): boolean {
  return !code || /^(und|unk|unknown|mis|zxx|mul)$/i.test(code);
}

/** "eng" / "en" / "fre" / "pt-BR" → "English" / "French" / "Portuguese (Brazil)". */
export function languageName(code: string | null | undefined): string {
  if (isUnknownLanguage(code)) return "Unknown";
  const raw = code!.trim();
  const lower = raw.toLowerCase().replace("_", "-");
  const base = lower.split("-")[0]!;
  return intlName(B2T[base] ? B2T[base]! + lower.slice(base.length) : lower) ?? LANGS[lower] ?? LANGS[base] ?? raw.toUpperCase();
}

/** "eng" / "fre" / "English" → "en" / "fr" (ISO 639-1 when we know one; else the input lower-cased). */
export function iso1(code: string | null | undefined): string {
  if (isUnknownLanguage(code)) return "";
  const lower = code!.trim().toLowerCase().replace("_", "-").split("-")[0]!;
  if (lower.length === 2) return lower;
  const name = LANGS[lower] ?? Object.keys(LANGS).map((k) => LANGS[k]!).find((n) => n.toLowerCase() === lower);
  if (!name) return lower;
  for (const k in LANGS) if (k.length === 2 && LANGS[k] === name) return k;
  return lower;
}

/** Languages offered when adding a preference (ISO 639-1), most common first-ish. */
export const PICKER_LANGUAGES: string[] = [
  "en", "fr", "de", "es", "it", "pt", "nl", "sv", "da", "no", "fi", "pl", "cs", "hu", "ro", "el", "tr", "ru", "uk",
  "ar", "he", "fa", "hi", "ja", "ko", "zh", "th", "vi", "id", "ms", "bg", "hr", "sr", "sk", "sl", "is",
];

/** Stable key for "same language" comparisons across 2- and 3-letter codes. */
export function languageKey(code: string | null | undefined): string {
  return isUnknownLanguage(code) ? "" : languageName(code).toLowerCase();
}

const SHORT_CODEC: Record<string, string> = {
  aac: "AAC", ac3: "AC3", eac3: "E-AC3", truehd: "TrueHD", dts: "DTS", "dts-hd": "DTS-HD", dtshd: "DTS-HD",
  flac: "FLAC", opus: "Opus", mp3: "MP3", mp2: "MP2", vorbis: "Vorbis", pcm_s16le: "PCM", pcm_s24le: "PCM", alac: "ALAC",
  subrip: "SRT", srt: "SRT", ass: "ASS", ssa: "SSA", webvtt: "WebVTT", vtt: "WebVTT", mov_text: "Text",
  hdmv_pgs_subtitle: "PGS", pgs: "PGS", dvd_subtitle: "VobSub", dvb_subtitle: "DVB",
};
export function codecShort(c: string | null | undefined): string {
  if (!c) return "";
  return SHORT_CODEC[c.toLowerCase()] ?? c.toUpperCase();
}

/** Looks like a file name (stream titles copied from the file): don't show it. */
function isFileName(t: string): boolean {
  return /\.(srt|ass|ssa|vtt|sub|idx|sup|smi|txt|mkv|mp4)$/i.test(t) || /[\[\]{}]/.test(t);
}
/** Whatever a title adds beyond language/codec/flags, or "" if it's redundant. */
function extraTitle(title: string | null | undefined, lang: string): string {
  if (!title || isFileName(title.trim())) return "";
  const rest = title
    .replace(new RegExp(lang.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "ig"), "")
    .replace(/\b(srt|subrip|ass|ssa|pgs|vobsub|webvtt|vtt|text|sdh|cc|hi|forced|full|default|subtitles?|subs?|aac|ac3|e-?ac3|dts(-hd)?|truehd|flac|stereo|mono|[257]\.1|2\.0)\b/gi, "")
    .replace(/[()\-–—_:·,.|/]+/g, " ")
    .trim();
  return rest ? title.trim() : "";
}

export interface TrackLabel {
  /** Main line, e.g. "English" or "English — Commentary". */
  title: string;
  /** Small print, e.g. "SDH · External · SRT". */
  detail: string;
  /** Compact form for buttons, e.g. "English SDH". */
  short: string;
}

/** "English · AAC 5.1", or "AAC 5.1" when the language is unknown. */
export function audioLabel(a: Pick<AudioStreamInfo, "codec" | "channels" | "language" | "title"> & { default?: boolean }): TrackLabel {
  const tech = `${codecShort(a.codec)} ${channelsLabel(a.channels)}`.trim();
  const known = !isUnknownLanguage(a.language);
  const lang = known ? languageName(a.language) : "";
  const extra = extraTitle(a.title, lang);
  const main = known ? `${lang} · ${tech}` : tech;
  return {
    title: extra ? `${main} — ${extra}` : main,
    detail: a.default ? "Default" : "",
    short: main,
  };
}

/** Flags SDH/HI from stream flags or filename/title tokens (`.hi.`, `.sdh.`, `cc`). */
export function isSdh(s: { title?: string | null }): boolean {
  const t = s.title ?? "";
  return /(^|[.\s_\-[(])(hi|sdh|cc)([.\s_\-\])]|$)/i.test(t) || /hearing[\s-]?impaired|\bsdh\b/i.test(t);
}

/** Language name + SDH/Forced, external/embedded and codec as small print. */
export function subtitleLabel(s: Pick<SubtitleStreamInfo, "language" | "title" | "forced" | "external"> & { codec?: string | null }): TrackLabel {
  let code = s.language;
  if (isUnknownLanguage(code) && s.title) {
    // External files often only carry the language in the name: "Movie.en.hi.srt".
    const m = /[._-]([a-z]{2,3})(?:[._-](?:hi|sdh|cc|forced))*\.(?:srt|ass|ssa|vtt|sub|sup)$/i.exec(s.title);
    if (m && !isUnknownLanguage(m[1]) && languageName(m[1]) !== m[1]!.toUpperCase()) code = m[1];
  }
  const lang = languageName(code);
  const sdh = isSdh(s);
  const forced = s.forced || /\bforced\b/i.test(s.title ?? "");
  const extra = extraTitle(s.title, lang);
  const detail = [sdh ? "SDH" : "", forced ? "Forced" : "", s.external ? "External" : "Embedded", codecShort(s.codec)].filter(Boolean).join(" · ");
  const short = `${lang}${sdh ? " SDH" : ""}${forced ? " (Forced)" : ""}`;
  return { title: extra ? `${lang} — ${extra}` : lang, detail, short };
}

/** Short technical tags for a file, e.g. ["4K", "Dolby Vision", "HEVC", "5.1"]. */
export function fileTags(f: MediaFile | undefined): string[] {
  if (!f) return [];
  const out: string[] = [];
  if (f.video) {
    out.push(resolutionLabel(f.video));
    const h = hdrLabel(f.video.hdr);
    if (h) out.push(h);
    out.push(codecName(f.video.codec));
  }
  const a = f.audio.find((x) => x.default) ?? f.audio[0];
  if (a) out.push(`${codecShort(a.codec)} ${channelsLabel(a.channels)}`);
  return out.filter(Boolean);
}

export function episodeCode(i: Pick<ItemSummary, "season_no" | "episode_no">): string {
  if (i.season_no == null && i.episode_no == null) return "";
  return `S${i.season_no ?? 0} · E${i.episode_no ?? 0}`;
}

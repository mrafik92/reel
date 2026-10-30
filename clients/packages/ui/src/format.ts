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
  sv: "Swedish", swe: "Swedish", da: "Danish", dan: "Danish", no: "Norwegian", nor: "Norwegian",
  pl: "Polish", pol: "Polish", ru: "Russian", rus: "Russian", zh: "Chinese", chi: "Chinese", zho: "Chinese",
  ar: "Arabic", ara: "Arabic", hi: "Hindi", hin: "Hindi", tr: "Turkish", tur: "Turkish", und: "Unknown",
};
export function languageName(code: string | null | undefined): string {
  if (!code) return "Unknown";
  return LANGS[code.toLowerCase()] ?? code.toUpperCase();
}

export function audioLabel(a: AudioStreamInfo): { title: string; detail: string } {
  return {
    title: a.title || languageName(a.language),
    detail: `${a.title ? languageName(a.language) + " · " : ""}${codecName(a.codec)} ${channelsLabel(a.channels)}`,
  };
}

export function subtitleLabel(s: SubtitleStreamInfo): { title: string; detail: string } {
  const flags = [s.forced ? "Forced" : "", s.external ? "External" : ""].filter(Boolean).join(" · ");
  return {
    title: s.title || languageName(s.language),
    detail: `${s.title ? languageName(s.language) + " · " : ""}${codecName(s.codec)}${flags ? " · " + flags : ""}`,
  };
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
  if (a) out.push(`${codecName(a.codec)} ${channelsLabel(a.channels)}`);
  return out.filter(Boolean);
}

export function episodeCode(i: Pick<ItemSummary, "season_no" | "episode_no">): string {
  if (i.season_no == null && i.episode_no == null) return "";
  return `S${i.season_no ?? 0} · E${i.episode_no ?? 0}`;
}

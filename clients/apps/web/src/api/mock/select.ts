import type { MediaFile } from "@reel/ui";
import { DEFAULT_PREFS, type PlaybackPrefs, type Selected } from "../prefs";
import { previewSelection } from "../../lib/tracks";
import type { DecideResponse, Quality, QualityOption, VariantOption } from "../types";

/** Mock of the server's automatic track choice (`selected`), from the mock-mode preferences. */
export function mockSelected(f: MediaFile, audioReq: number | null | undefined, subReq: number | null | undefined, forCast = false): Selected {
  let p: PlaybackPrefs = DEFAULT_PREFS;
  try {
    const v = localStorage.getItem("reel.mock.prefs");
    if (v) p = { ...DEFAULT_PREFS, ...(JSON.parse(v) as Partial<PlaybackPrefs>) };
  } catch {
    /* defaults */
  }
  const auto = previewSelection(f.audio, f.subtitles, p);
  const why: string[] = [];
  const audio = audioReq ?? auto.audio;
  why.push(audioReq != null ? "audio chosen by the client" : p.audio_languages[0] === "original" ? "audio: original language" : "audio: preferred language");
  let sub: number | null;
  if (subReq != null && subReq >= 0) {
    sub = subReq;
    why.push("subtitle chosen by the client");
  } else if (subReq === -1) {
    sub = null;
    why.push("subtitles turned off by the client");
  } else {
    sub = auto.sub;
    why.push(sub != null ? `subtitles: ${p.subtitle_mode === "auto" ? "audio isn't in one of your languages" : "your subtitle languages"}` : "subtitles: not needed");
  }
  if (forCast) why.push("for a cast receiver");
  return { audio_index: audio, subtitle_index: sub, reason: why.join("; ") };
}

// ---- quality (#87) -----------------------------------------------------------------------------

type Decision = DecideResponse["decision"];

/** Mock user bitrate limit (kbit/s): `localStorage["reel.mock.maxBitrate"]`, unset = unlimited. */
export function mockUserCap(): number | null {
  try {
    const v = Number(localStorage.getItem("reel.mock.maxBitrate"));
    return v > 0 ? v : null;
  } catch {
    return null;
  }
}

const RUNGS: { q: Quality; w: number; h: number; v: number; a: number }[] = [
  { q: "1080p", w: 1920, h: 1080, v: 8000, a: 256 },
  { q: "720p", w: 1280, h: 720, v: 4000, a: 160 },
  { q: "480p", w: 854, h: 480, v: 2000, a: 128 },
];
const even = (n: number) => Math.round(n / 2) * 2;
const mbps = (k: number) => `${(k / 1000).toFixed(1)} Mbps`;

/**
 * Mock of the server's `decide_quality` (`server/crates/playback/src/quality.rs`): resolves the
 * requested quality against the user's limit, rewriting `base` (the compatibility decision) into a
 * ladder rung when needed. Same rules, simplified: never upscale, Original only yields to the limit.
 */
export function mockQuality(
  src: { width: number; height: number; hdr: string; kbps: number | null },
  req: { quality?: Quality | null; max_bitrate_kbps?: number | null },
  base: Decision,
): { mode: Decision["mode"]; decision: Decision; quality: Quality; bitrate_kbps: number; quality_options: QualityOption[] } {
  const baseEnc = base.video.type === "encode";
  const orig = baseEnc ? 8000 + 192 : src.kbps ?? 0;
  const q = req.quality ?? "auto";
  const userCap = mockUserCap();
  const hint = q === "original" ? null : req.max_bitrate_kbps ?? null;
  const caps = [userCap, hint].filter((x): x is number => x != null && x > 0);
  const cap = caps.length ? Math.min(...caps) : null;
  const fits = (k: number) => cap == null || k <= cap;
  const total = (r: (typeof RUNGS)[number]) => Math.min(r.v, src.kbps ?? r.v) + r.a;
  const ok = (r: (typeof RUNGS)[number]) =>
    src.height > 0 &&
    (src.width >= r.w * 0.9 || src.height >= r.h * 0.9) &&
    !(fits(orig) && src.width <= r.w && src.height <= r.h && src.kbps != null && orig <= r.v + r.a) &&
    (userCap == null || total(r) <= userCap || r.q === "480p");

  let pick: number | undefined;
  let why = "";
  if (fits(orig)) {
    const i = RUNGS.findIndex((r) => r.q === q);
    if (i >= 0 && ok(RUNGS[i]!) && fits(total(RUNGS[i]!))) {
      pick = i;
      why = `quality ${q} requested`;
    }
  } else {
    const from = Math.max(0, RUNGS.findIndex((r) => r.q === q));
    const usable = RUNGS.map((_, i) => i).filter((i) => i >= from && ok(RUNGS[i]!));
    pick = usable.find((i) => fits(total(RUNGS[i]!))) ?? usable[usable.length - 1];
    if (pick != null) why = `source needs ~${mbps(orig)} but the cap is ${mbps(cap ?? 0)} (${hint != null && hint === cap ? "client bandwidth" : "user limit"})`;
  }

  let decision = base;
  let quality: Quality = "original";
  let rate = orig;
  if (pick != null) {
    const r = RUNGS[pick]!;
    const s = Math.min(r.w / (src.width || r.w), r.h / (src.height || r.h), 1);
    let v = Math.min(r.v, src.kbps ?? r.v);
    if (cap != null) v = Math.min(v, Math.floor(cap * 0.95) - r.a);
    v = Math.max(300, v);
    const audio: Decision["audio"] = base.audio.type === "none" ? base.audio : r.q !== "1080p" ? { type: "encode", codec: "aac", channels: 2, bitrate_kbps: r.a } : base.audio;
    decision = {
      ...base,
      mode: "transcode",
      video: { type: "encode", codec: "h264", width: even((src.width || r.w) * s), height: even((src.height || r.h) * s), bitrate_kbps: v, tonemap: src.hdr !== "none" },
      audio,
      reasons: base.reasons.concat(`adaptive quality ${r.q}: ${why}`),
    };
    quality = r.q;
    rate = v + (audio.type === "encode" ? audio.bitrate_kbps : 192);
  } else if (baseEnc && base.video.type === "encode") {
    const w = base.video.width;
    quality = w * base.video.height + 1 >= src.width * src.height ? "original" : w >= 1500 ? "1080p" : w >= 1000 ? "720p" : "480p";
  }
  const options: QualityOption[] = [
    { id: "auto", label: "Auto", bitrate_kbps: rate, available: true },
    { id: "original", label: orig ? `Original (${mbps(orig)})` : "Original", bitrate_kbps: orig, available: userCap == null || orig <= userCap },
    ...RUNGS.map((r) => ({ id: r.q, label: `${r.q} (${mbps(total(r))})`, bitrate_kbps: total(r), available: ok(r) })),
  ];
  return { mode: decision.mode, decision, quality, bitrate_kbps: rate, quality_options: options };
}

/** The mock multi-variant HLS (`vite.config.ts`): ids, sizes and the master's bit rates (kbit/s). */
const MOCK_VARIANTS: VariantOption[] = [
  { id: "480p", label: "480p", width: 854, height: 480, bandwidth_kbps: 816, average_kbps: 696, codecs: "avc1.4d4028,mp4a.40.2", transcode: true },
  { id: "720p", label: "720p", width: 1280, height: 720, bandwidth_kbps: 1536, average_kbps: 1296, codecs: "avc1.4d4028,mp4a.40.2", transcode: true },
  { id: "original", label: "Original", width: 1920, height: 1080, bandwidth_kbps: 3096, average_kbps: 2596, codecs: "avc1.4d4028,mp4a.40.2", transcode: false },
];

/**
 * Mock of the server's adaptive plan (#111, `playback/src/variants.rs`), simplified: an adaptive
 * request gets the mock 3-variant master unless the file stays direct play (Q1: estimate ≥ 1.5×
 * its bitrate, or a quality other than Auto). Transcode-only titles list 1080p instead of the
 * original. Start: Auto → highest peak ≤ 0.8 × estimate (lowest with none), Original → original,
 * a rung → the highest variant no taller than it.
 */
export function mockAdaptive(
  req: { adaptive?: boolean | null; bandwidth_kbps?: number | null; quality?: Quality | null },
  mode: Decision["mode"],
  fileKbps: number | null,
): { url: string; variants: VariantOption[]; start_variant: Quality; mode: Decision["mode"] } | undefined {
  if (!req.adaptive) return undefined;
  const q = req.quality ?? "auto";
  const est = req.bandwidth_kbps && req.bandwidth_kbps > 0 ? req.bandwidth_kbps : null;
  let m = mode;
  if (mode === "direct_play") {
    if (q !== "auto" || (est != null && fileKbps != null && est >= fileKbps * 1.5)) return undefined;
    m = "remux";
  }
  const transcode = m === "transcode";
  const variants = MOCK_VARIANTS.map((v) => (transcode && v.id === "original" ? { ...v, id: "1080p" as Quality, label: "1080p", transcode: true } : v));
  let start = variants[0]!;
  if (q === "auto") {
    if (est != null) start = [...variants].reverse().find((v) => v.bandwidth_kbps <= est * 0.8) ?? variants[0]!;
  } else if (q === "original") {
    start = variants[variants.length - 1]!;
  } else {
    const h = q === "1080p" ? 1080 : q === "720p" ? 720 : 480;
    start = [...variants].reverse().find((v) => v.height <= h) ?? variants[0]!;
  }
  return { url: transcode ? "/mock-media/abr/master-transcode.m3u8" : "/mock-media/abr/master.m3u8", variants, start_variant: start.id, mode: m };
}

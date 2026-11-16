import type { MediaFile } from "@reel/ui";
import { DEFAULT_PREFS, type PlaybackPrefs, type Selected } from "../prefs";
import { previewSelection } from "../../lib/tracks";
import type { DecideResponse, Quality, QualityOption } from "../types";

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

import { createSignal } from "solid-js";
import { getServerUrl, type VariantSelect } from "@reel/ui";
import type { DecideResponse, Quality, VariantOption } from "../api";

/**
 * Adaptive streaming on the client (#111 ③, the rest of #1). The server lists the variants of a
 * session (original + rungs) and hls.js switches between them per segment; this module holds:
 *
 * - the **bandwidth estimate**: per device and per server, kept 3 days, fed from real media
 *   downloads (hls.js fragments, and a short ranged probe during direct play). Drops count more than
 *   recoveries, so a congested link is noticed quickly. Decide gets it as `bandwidth_kbps` (it picks
 *   the start variant and direct play vs remux; it caps nothing) and hls.js is seeded with it;
 * - the **quality mapping** (#87 on variants): Auto = ABR over every variant, Original = pin the
 *   original, a rung = ABR capped at that size. A choice the master can't meet re-decides;
 * - the **direct-play fallback**: two stalls in 30 s under Auto re-decide from the playhead, which
 *   remuxes with rungs instead.
 *
 * Salvaged from PR #69 (`lib/abr.ts`); its step-down/step-up re-decide controller is gone, since
 * hls.js now does that per segment without a new session.
 */

// ---- bandwidth estimate ------------------------------------------------------------------

interface Stored {
  kbps: number;
  at: number;
}

const MAX_AGE_MS = 3 * 24 * 3600_000;
const SAVE_EVERY_MS = 5000;

/** Storage key: one estimate per server (a LAN and a remote server differ). */
export function estimateKey(server = getServerUrl()): string {
  return `reel.bw.${server || (typeof location !== "undefined" ? location.host : "local")}`;
}

function load(now = Date.now()): Stored | undefined {
  try {
    const v = JSON.parse(localStorage.getItem(estimateKey()) || "null") as Stored | null;
    return v && v.kbps > 0 && now - v.at < MAX_AGE_MS ? v : undefined;
  } catch {
    return undefined;
  }
}

const initial = load();
const [estimate, setEstimate] = createSignal<number | undefined>(initial?.kbps);
const [measuredAt, setMeasuredAt] = createSignal<number | undefined>(initial?.at);
/** Current connection estimate (kbit/s) on this device, or undefined before the first measurement. */
export { estimate as bandwidthEstimate };
/** When the estimate last took a sample (ms since epoch). */
export { measuredAt as bandwidthMeasuredAt };

/**
 * Next estimate after a sample: a rolling average leaning on recent samples. A drop weighs 0.35, a
 * recovery 0.2 (times `weight`, at most 0.6), so a congested link shows within a few fragments.
 */
export function nextEstimate(cur: number | undefined, kbps: number, weight = 1): number {
  if (cur == null) return kbps;
  const a = Math.min(0.6, (kbps < cur ? 0.35 : 0.2) * weight);
  return cur * (1 - a) + kbps * a;
}

let lastSave = 0;
function save(kbps: number, now: number, force = false): void {
  if (!force && now - lastSave < SAVE_EVERY_MS) return;
  lastSave = now;
  try {
    localStorage.setItem(estimateKey(), JSON.stringify({ kbps: Math.round(kbps), at: now }));
  } catch {
    /* storage disabled: the estimate lasts until reload */
  }
}

/** Feed a throughput sample (kbit/s). */
export function recordThroughput(kbps: number, weight = 1, now = Date.now()): void {
  if (!(kbps > 0) || !isFinite(kbps)) return;
  const next = nextEstimate(estimate(), kbps, weight);
  setEstimate(next);
  setMeasuredAt(now);
  save(next, now);
}

/**
 * The link couldn't carry `kbps` (direct play stalled): hold the estimate below it so the next
 * decide doesn't pick direct play again (Q1 needs ≥ 1.5×). hls.js measures the real speed from there.
 */
export function capEstimate(kbps: number, now = Date.now()): void {
  if (!(kbps > 0)) return;
  const cur = estimate();
  const next = Math.min(cur ?? kbps, kbps);
  setEstimate(next);
  setMeasuredAt(now);
  save(next, now, true);
}

/** `bandwidth_kbps` for decide: the estimate, rounded; omitted when there is none. */
export function estimateKbps(): number | undefined {
  const e = estimate();
  return e && e > 0 ? Math.round(e) : undefined;
}

/** Probe again during direct play only when the estimate is older than this. */
const PROBE_AFTER_MS = 10 * 60_000;

export function probeDue(now = Date.now()): boolean {
  const at = measuredAt();
  return at == null || now - at > PROBE_AFTER_MS;
}

/**
 * Short ranged download of a media URL (≈2 MB, at most ~3 s) → kbit/s, timed from the first byte so
 * server latency doesn't read as a slow link. Records the sample; undefined when it can't tell.
 * Used during direct play, where hls.js isn't there to measure.
 */
export async function probeThroughput(url: string, bytes = 2 * 1024 * 1024, capMs = 3000): Promise<number | undefined> {
  if (typeof fetch === "undefined" || typeof AbortController === "undefined") return undefined;
  const ctl = new AbortController();
  const t0 = performance.now();
  let first = 0;
  let got = 0;
  const timer = setTimeout(() => ctl.abort(), capMs + 1500);
  try {
    const res = await fetch(url, { headers: { Range: `bytes=0-${bytes - 1}` }, signal: ctl.signal, cache: "no-store" });
    if (!res.ok || !res.body) return undefined;
    const reader = res.body.getReader();
    first = performance.now();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      got += value.byteLength;
      if (got >= bytes || performance.now() - first > capMs) {
        ctl.abort();
        break;
      }
    }
  } catch {
    /* aborted on purpose, or a network error */
  } finally {
    clearTimeout(timer);
  }
  const ms = performance.now() - (first || t0);
  if (got < 256 * 1024 || ms < 50) return undefined;
  const kbps = (got * 8) / ms; // bytes·8 / ms = kbit/s
  recordThroughput(kbps, 1.5);
  return kbps;
}

// ---- quality → variants ------------------------------------------------------------------

/** A multi-variant session: the player switches variants itself. */
export function isAdaptive(d: DecideResponse | undefined): boolean {
  return !!d?.start_variant && (d.variants?.length ?? 0) > 1;
}

type Rung = "1080p" | "720p" | "480p";
const RUNGS: Rung[] = ["480p", "720p", "1080p"];
/** Bounding box of a ladder class (a 2.39:1 1080p film is 1920×804: still 1080p). */
const BOX: Record<Rung, [number, number]> = { "480p": [854, 480], "720p": [1280, 720], "1080p": [1920, 1080] };
/** Encoders round sizes to even numbers and some sources are a few pixels over: allow for it. */
const SLACK = 8;

/** Size class of a variant: a rung's own id, or the smallest box the original fits (4K → none). */
export function variantClass(v: Pick<VariantOption, "id" | "width" | "height">): Rung | undefined {
  if (v.id !== "original" && v.id !== "auto") return v.id;
  if (!v.width || !v.height) return undefined;
  return RUNGS.find((r) => v.width <= BOX[r][0] + SLACK && v.height <= BOX[r][1] + SLACK);
}

/**
 * The selection that applies `q` to this session's variants, or `null` when the master can't
 * meet it: Original with no `original` variant, or a rung with no variant of that size (the
 * app re-decides; a single-variant session at that rung is what #87 did).
 */
export function variantSelect(q: Quality, variants: VariantOption[] | undefined): VariantSelect | null {
  const vs = variants ?? [];
  if (q === "auto") return {};
  if (q === "original") return vs.some((v) => v.id === "original") ? { pin: "original" } : null;
  const rank = RUNGS.indexOf(q);
  if (!vs.some((v) => variantClass(v) === q)) return null;
  // Everything no bigger than the choice (never upscales: rungs are never above the source).
  const allow = vs.filter((v) => {
    const c = variantClass(v);
    return c != null && RUNGS.indexOf(c) <= rank;
  });
  return { allow: allow.map((v) => v.id) };
}

/** The variant with this id. */
export function variantOf(d: DecideResponse | undefined, id: string | undefined): VariantOption | undefined {
  return id ? d?.variants?.find((v) => v.id === id) : undefined;
}

// ---- direct play: stalls → adaptive -----------------------------------------------------

/**
 * Counts stalls during direct play (#1). Two within 30 s mean the link can't carry the file: the
 * player re-decides from the playhead, which gives a remux original with rungs. Waits right after a
 * start or a seek aren't stalls.
 */
export class StallCounter {
  private stalls: number[] = [];
  private quietUntil = 0;
  constructor(private readonly windowMs = 30_000, private readonly needed = 2, private readonly graceMs = 3000) {}

  /** Playback (re)started or the viewer seeked: the next wait is expected. */
  settle(now = Date.now()): void {
    this.quietUntil = now + this.graceMs;
  }

  /** The player is waiting for data while meant to play. True when it's time to switch. */
  stall(now = Date.now()): boolean {
    if (now < this.quietUntil) return false;
    this.stalls = this.stalls.filter((t) => now - t < this.windowMs);
    this.stalls.push(now);
    if (this.stalls.length < this.needed) return false;
    this.stalls = [];
    return true;
  }
}

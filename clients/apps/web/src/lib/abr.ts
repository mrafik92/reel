import { createSignal } from "solid-js";
import { getServerUrl } from "@reel/ui";
import type { DecideResponse } from "../api";

/**
 * Adaptive quality: a bandwidth estimate measured from real media downloads (persisted per
 * server), and the step-down / step-up rules for "Auto". The server does the encoding; we pick
 * the rung (`quality`) and send a bandwidth hint (`max_bitrate_kbps`) on decide / vod play.
 */

export type QualityId = "auto" | "original" | "1080p" | "720p" | "480p";
export interface QualityOption {
  id: QualityId;
  label: string;
  bitrate_kbps: number;
  available: boolean;
}
export interface QualityInfo {
  /** Resolved: `original` or a rung (never `auto`). */
  quality: QualityId;
  /** Expected bitrate of this stream. */
  bitrate_kbps: number;
  options: QualityOption[];
}

/** Ladder, best first. */
export const LADDER: QualityId[] = ["original", "1080p", "720p", "480p"];
export const QUALITY_NAME: Record<QualityId, string> = {
  auto: "Auto",
  original: "Original",
  "1080p": "1080p",
  "720p": "720p",
  "480p": "480p",
};

/** The quality fields of a decide / vod play answer (older servers omit them). */
export function qualityOf(d: DecideResponse | undefined): QualityInfo | undefined {
  const x = d as (DecideResponse & { quality?: QualityId; bitrate_kbps?: number; quality_options?: QualityOption[] }) | undefined;
  if (!x || !x.quality || !x.quality_options) return undefined;
  return { quality: x.quality, bitrate_kbps: x.bitrate_kbps ?? 0, options: x.quality_options };
}

export function fmtMbps(kbps: number | null | undefined): string {
  if (!kbps || !isFinite(kbps)) return "—";
  return kbps >= 10_000 ? `${Math.round(kbps / 1000)} Mbps` : `${(kbps / 1000).toFixed(1)} Mbps`;
}

// ---- bandwidth estimate ------------------------------------------------------------------

interface Stored {
  kbps: number;
  at: number;
}
const MAX_AGE = 3 * 24 * 3600_000;
const key = () => `reel.bw.${getServerUrl() || (typeof location !== "undefined" ? location.host : "local")}`;

function load(): Stored | undefined {
  try {
    const v = JSON.parse(localStorage.getItem(key()) || "null") as Stored | null;
    return v && v.kbps > 0 && Date.now() - v.at < MAX_AGE ? v : undefined;
  } catch {
    return undefined;
  }
}

const [estimate, setEstimate] = createSignal<number | undefined>(load()?.kbps);
/** Current connection estimate (kbit/s), shared by every player on this device. */
export { estimate as bandwidthEstimate };

let lastSave = 0;
/**
 * Feed a throughput sample (kbit/s). Rolling average leaning on recent samples; big drops
 * count faster than recoveries so a congested link is noticed quickly.
 */
export function recordThroughput(kbps: number, weight = 1): void {
  if (!(kbps > 0) || !isFinite(kbps)) return;
  const cur = estimate();
  const a = Math.min(0.6, (cur == null ? 1 : kbps < cur ? 0.35 : 0.2) * weight);
  const next = cur == null ? kbps : cur * (1 - a) + kbps * a;
  setEstimate(next);
  const now = Date.now();
  if (now - lastSave > 5000) {
    lastSave = now;
    try {
      localStorage.setItem(key(), JSON.stringify({ kbps: Math.round(next), at: now }));
    } catch {
      /* storage disabled */
    }
  }
}

/** Is the stored estimate fresh enough to skip a probe? */
export function hasEstimate(): boolean {
  return estimate() != null;
}

/**
 * Short ranged download of a media URL (≈2 MB, at most ~3 s) → kbit/s, measured from the first
 * byte so server start-up latency doesn't count as a slow link. Undefined when it can't tell.
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
    /* aborted on purpose, or network error */
  } finally {
    clearTimeout(timer);
  }
  const ms = performance.now() - (first || t0);
  if (got < 256 * 1024 || ms < 50) return undefined;
  const kbps = (got * 8) / ms; // bytes·8 / ms = kbit/s
  recordThroughput(kbps, 1.5);
  return kbps;
}

// ---- Auto rules --------------------------------------------------------------------------

export interface AbrStep {
  dir: "down" | "up";
  target: QualityId;
  reason: string;
}

/**
 * Step-down when the buffer underruns twice in 30 s, or runs below 3 s while the link carries
 * less than 1.2× the stream; step-up after 2 min of a healthy buffer when the link carries 2×
 * the next rung. Never more than one switch per 30 s.
 */
export class AbrController {
  private stalls: number[] = [];
  private startedAt = 0;
  private lastSwitch = -Infinity;
  private healthySince: number | null = null;
  private info?: QualityInfo;

  constructor(private now: () => number = () => Date.now()) {}

  /** A new stream started (initial load or after a switch). */
  reset(info: QualityInfo | undefined, switched: boolean): void {
    this.info = info;
    this.stalls = [];
    this.healthySince = null;
    this.startedAt = this.now();
    if (switched) this.lastSwitch = this.now();
  }

  stall(): void {
    const t = this.now();
    this.stalls.push(t);
    this.stalls = this.stalls.filter((s) => t - s < 30_000);
    this.healthySince = null;
  }

  private ladder(): QualityOption[] {
    const opts = this.info?.options ?? [];
    return LADDER.map((id) => opts.find((o) => o.id === id && o.available)).filter((o): o is QualityOption => !!o);
  }

  evaluate(bufferAhead: number, playing: boolean, throughput: number | undefined): AbrStep | null {
    const info = this.info;
    if (!info) return null;
    const t = this.now();
    if (t - this.lastSwitch < 30_000) return null;
    const ladder = this.ladder();
    const i = ladder.findIndex((o) => o.id === info.quality);
    if (i < 0) return null;
    const stream = info.bitrate_kbps || ladder[i]!.bitrate_kbps;

    // ---- down
    this.stalls = this.stalls.filter((s) => t - s < 30_000);
    const starving = playing && t - this.startedAt > 8000 && bufferAhead < 3 && throughput != null && throughput < stream * 1.2;
    if ((this.stalls.length >= 2 || starving) && i < ladder.length - 1) {
      // Skip rungs the link clearly can't carry, but always move at least one.
      let j = i + 1;
      while (throughput != null && j < ladder.length - 1 && ladder[j]!.bitrate_kbps > throughput * 0.7) j++;
      return {
        dir: "down",
        target: ladder[j]!.id,
        reason: this.stalls.length >= 2 ? "playback stalled twice in 30 s" : `buffer ${bufferAhead.toFixed(1)} s, link ${fmtMbps(throughput)} < 1.2× ${fmtMbps(stream)}`,
      };
    }

    // ---- up
    if (bufferAhead > 25 && playing) {
      if (this.healthySince == null) this.healthySince = t;
    } else if (bufferAhead < 15) {
      this.healthySince = null;
    }
    if (this.healthySince != null && t - this.healthySince >= 120_000 && i > 0) {
      const next = ladder[i - 1]!;
      if (throughput != null && throughput >= next.bitrate_kbps * 2) {
        return { dir: "up", target: next.id, reason: `healthy for 2 min, link ${fmtMbps(throughput)} ≥ 2× ${fmtMbps(next.bitrate_kbps)}` };
      }
    }
    return null;
  }
}

// ---- preference ------------------------------------------------------------------------

const PREF_KEY = "reel.quality";
/** Manual choice for this browser session (until reload); "remember" stores it on the device. */
let sessionChoice: QualityId | undefined;

export function preferredQuality(): QualityId {
  if (sessionChoice) return sessionChoice;
  try {
    const v = localStorage.getItem(PREF_KEY) as QualityId | null;
    if (v && (v === "auto" || LADDER.indexOf(v) >= 0)) return v;
  } catch {
    /* ignore */
  }
  return "auto";
}
export function rememberedQuality(): boolean {
  try {
    return localStorage.getItem(PREF_KEY) != null;
  } catch {
    return false;
  }
}
export function setPreferredQuality(q: QualityId, remember: boolean): void {
  sessionChoice = q;
  try {
    if (remember) localStorage.setItem(PREF_KEY, q);
    else localStorage.removeItem(PREF_KEY);
  } catch {
    /* ignore */
  }
}

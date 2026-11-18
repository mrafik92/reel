/**
 * Multi-variant HLS on hls.js (#111): which level to start on, pin or cap, and the per-fragment
 * throughput samples the app's bandwidth estimate is fed from. Pure logic, no hls.js or DOM, so it
 * can be tested on its own; `HlsJsAdapter` applies the results.
 *
 * Levels are matched to the server's variants by URL path (`…/{variant}/index.m3u8`), never by
 * index: hls.js drops levels whose `CODECS` the browser rejects, and sorts the rest by bandwidth.
 */

/** Which variants the player may play. */
export interface VariantSelect {
  /** Play only this variant (an hls.js manual level). Wins over `allow`. */
  pin?: string;
  /** ABR over these variants only (all of them when omitted). */
  allow?: string[];
}

/** What `HlsJsAdapter.load` needs for a multi-variant session. */
export interface VariantOptions {
  /** The variant the server prestarted; hls.js starts there. */
  start?: string;
  /** Seed for hls.js's bandwidth estimate (bit/s): the app's stored estimate. */
  estimateBps?: number;
  select?: VariantSelect;
}

/** The variant id of a level URL (`…/stream/{sid}/720p/index.m3u8?sig=…` → `720p`). */
export function variantIdOf(url: string | undefined): string | undefined {
  if (!url) return undefined;
  const m = /\/([^/?#]+)\/[^/?#]+\.m3u8(?:[?#]|$)/.exec(url);
  return m ? decodeURIComponent(m[1]!) : undefined;
}

/** hls.js level settings for a selection. `-1` means "not set" for each field. */
export interface LevelPlan {
  /** `hls.startLevel`. */
  start: number;
  /** Manual level (`hls.loadLevel` / `nextLevel`), or -1 for ABR. */
  pin: number;
  /** `hls.autoLevelCapping`, or -1 for no cap. */
  cap: number;
}

/**
 * Turn a selection into level indexes. `ids` are the variant ids of `hls.levels`, in hls.js's order
 * (ascending bandwidth). Returns `undefined` when the selection can't be met by these levels
 * (the pinned variant is missing, or none of the allowed ones is there): the app then re-decides.
 */
export function planLevels(ids: (string | undefined)[], o: { start?: string; select?: VariantSelect }): LevelPlan | undefined {
  if (!ids.length) return undefined;
  const sel = o.select ?? {};
  let pin = -1;
  let cap = -1;
  if (sel.pin) {
    pin = ids.indexOf(sel.pin);
    if (pin < 0) return undefined;
  } else if (sel.allow) {
    for (let i = 0; i < ids.length; i++) if (ids[i] && sel.allow.indexOf(ids[i]!) >= 0) cap = i;
    if (cap < 0) return undefined;
    // Allowing everything is no cap at all.
    if (cap === ids.length - 1) cap = -1;
  }
  let start = o.start ? ids.indexOf(o.start) : -1;
  if (pin >= 0) start = pin;
  else if (start < 0) start = 0; // start low, then climb
  else if (cap >= 0 && start > cap) start = cap;
  return { start, pin, cap };
}

/** The part of hls.js's fragment stats a sample needs. */
export interface FragStats {
  loaded: number;
  loading: { start: number; first: number; end: number };
}

/** Fragments smaller than this say more about latency than throughput. */
const MIN_SAMPLE_BYTES = 192 * 1024;

/**
 * Throughput of one fragment download (kbit/s), timed from the first byte so a variant's ffmpeg
 * starting up (the server answers once the segment exists) doesn't read as a slow link. `undefined`
 * for fragments too small or too quick to tell.
 */
export function fragmentKbps(s: FragStats | undefined): number | undefined {
  if (!s || s.loaded < MIN_SAMPLE_BYTES) return undefined;
  const from = s.loading.first > 0 ? s.loading.first : s.loading.start;
  const ms = s.loading.end - from;
  if (!(ms >= 5) || !isFinite(ms)) return undefined;
  return (s.loaded * 8) / ms; // bytes·8 / ms = kbit/s
}

/**
 * hls.js settings for a multi-variant VOD session, on top of `hlsConfig`:
 * - `autoStartLoad: false`: the adapter sets the start level and pin/cap from the parsed master,
 *   then starts loading;
 * - `testBandwidth: false`: otherwise hls.js downloads a throwaway fragment of the lowest level,
 *   which would start that rung's ffmpeg on every play;
 * - `abrEwmaDefaultEstimate`: the app's stored estimate, so the first switch decisions use it;
 * - `preserveManualLevelOnError`: a pinned variant (Original) stays pinned when a fragment fails,
 *   instead of hls.js falling back to ABR and starting a rung the viewer asked not to get.
 */
export function abrConfig(o: VariantOptions): Record<string, unknown> {
  const c: Record<string, unknown> = { autoStartLoad: false, testBandwidth: false, preserveManualLevelOnError: true };
  if (o.estimateBps && o.estimateBps > 0 && isFinite(o.estimateBps)) c.abrEwmaDefaultEstimate = Math.round(o.estimateBps);
  return c;
}

/**
 * Where the preview card goes and how its row makes room for it (#301). Pure: the caller measures,
 * this decides. All values are viewport pixels.
 */

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** `ranked`: the numbered row. Its card opens at the poster's left edge, beside the numeral. */
export type PreviewKind = "poster" | "landscape" | "ranked";

/** Card width over tile width. Wide tiles get a smaller factor, or the card would cover two neighbours. */
export const POSTER_SCALE = 1.9;
export const LANDSCAPE_SCALE = 1.32;
/** Poster cards never get narrower than this, in rem: the body needs room for its line of facts. */
export const MIN_WIDTH_REM = 20;

export function previewWidth(kind: PreviewKind, tileWidth: number, rem: number, viewportWidth: number, gutter: number): number {
  const want = kind === "landscape" ? tileWidth * LANDSCAPE_SCALE : Math.max(tileWidth * POSTER_SCALE, MIN_WIDTH_REM * rem);
  return Math.max(0, Math.min(want, viewportWidth - 2 * gutter));
}

export interface PlacementInput {
  kind: PreviewKind;
  /** The tile's art (its frame) at its layout position: no transforms. */
  tile: Rect;
  width: number;
  height: number;
  /** Bottom edge of the row's heading; the card never covers it. `null` when the row has none. */
  headBottom: number | null;
  viewportWidth: number;
  viewportHeight: number;
  /** Side margins the card stays inside. */
  gutter: number;
  /** Lowest top the card may have (below a fixed top bar). */
  minTop: number;
  /** Space kept between the card and the heading, and between the card and the viewport's bottom. */
  gap: number;
}

export interface Placement {
  left: number;
  top: number;
  /** The tile's centre in the card's own coordinates: the card grows out of its tile. */
  originX: number;
  originY: number;
  /**
   * How far the tiles before and after the open one move along the row. `before` also moves the
   * open tile's own numeral in the numbered row. Negative is left.
   */
  before: number;
  after: number;
}

export function previewPlacement(p: PlacementInput): Placement {
  const t = p.tile;
  const cx = t.left + t.width / 2;
  const cy = t.top + t.height / 2;
  const anchor = p.kind === "ranked" ? t.left : cx - p.width / 2;
  const left = Math.max(p.gutter, Math.min(anchor, p.viewportWidth - p.gutter - p.width));

  let top = cy - p.height / 2;
  if (p.headBottom != null) top = Math.max(top, p.headBottom + p.gap);
  // The viewport wins over the heading: a row at the bottom of the screen shows the whole card.
  top = Math.max(p.minTop, Math.min(top, p.viewportHeight - p.height - p.gap));

  return {
    left,
    top,
    originX: cx - left,
    originY: cy - top,
    // The neighbours keep their own spacing to the card's edges, so nothing is covered.
    before: left - t.left,
    after: left + p.width - (t.left + t.width),
  };
}

/** The translateX part of a computed `transform` (`matrix(…)`/`matrix3d(…)`), 0 for `none`. */
export function translateXOf(transform: string | null | undefined): number {
  if (!transform || transform === "none") return 0;
  const m = /^matrix(3d)?\(([^)]*)\)$/.exec(transform.trim());
  if (!m) return 0;
  const v = m[2]!.split(",").map((s) => parseFloat(s));
  const x = m[1] ? v[12] : v[4];
  return x != null && isFinite(x) ? x : 0;
}

interface VideoLike {
  width: number;
  hdr: string;
}

/** `4K · HDR`, `4K`, `HDR`, or "" for ordinary HD/SDR (or nothing known), from a title's files. */
export function qualityBadge(files: ReadonlyArray<{ video?: VideoLike | null }> | null | undefined): string {
  let uhd = false;
  let hdr = false;
  for (const f of files ?? []) {
    const v = f.video;
    if (!v) continue;
    if (v.width >= 3800) uhd = true;
    if (v.hdr && v.hdr !== "none") hdr = true;
  }
  return [uhd ? "4K" : "", hdr ? "HDR" : ""].filter(Boolean).join(" · ");
}

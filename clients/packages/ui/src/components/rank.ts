/**
 * The numbered row's numerals (#301): SVG text whose outline is exactly as tall as the poster, in Archivo at width 62
 * and weight 900 (`fonts/archivo-rank-digits.woff2`), at the font's own proportions.
 *
 * Coordinates are in a viewBox 1000 units tall (the poster's height); the SVG's width follows from
 * `width`. The numbers below are the font's metrics, printed by `scripts/mkfont.py`.
 */

/** Advance widths of 0–9 per 1000 em. */
const ADVANCE = [444, 371, 427, 440, 435, 437, 445, 396, 432, 445];
/** Height of the digits' flat tops (1, 4, 7) per em. */
export const RANK_CAP = 0.688;
/** Round digits (0, 2, 3…) overshoot the flat ones: how far their ink runs above and below the baseline, per em. */
export const RANK_INK_TOP = 0.7;
export const RANK_INK_BOTTOM = 0.012;
/** The viewBox height. */
export const RANK_H = 1000;
/** Outline width, about 6 px on a 144 px poster. Half of it lies outside the glyph. */
export const RANK_STROKE = 42;
/** "10" and up are set tighter so they don't dominate the row. */
const TIGHTEN = -0.03;

export interface RankNumeral {
  /** viewBox width; the SVG's aspect ratio is `width / RANK_H`. */
  width: number;
  fontSize: number;
  letterSpacing: number;
  x: number;
  /** Baseline: the outline's bottom edge sits on the poster's bottom edge. */
  y: number;
}

export function rankNumeral(n: number): RankNumeral {
  const digits = String(Math.max(0, Math.floor(n)));
  // The tallest digits plus their outline fill the poster's height exactly, so nothing stands above its
  // top edge or below its bottom one.
  const fontSize = (RANK_H - RANK_STROKE) / (RANK_INK_TOP + RANK_INK_BOTTOM);
  const letterSpacing = digits.length > 1 ? TIGHTEN * fontSize : 0;
  let w = RANK_STROKE;
  for (let i = 0; i < digits.length; i++) {
    w += (ADVANCE[digits.charCodeAt(i) - 48]! / 1000) * fontSize + (i ? letterSpacing : 0);
  }
  return {
    width: Math.ceil(w),
    fontSize: Math.round(fontSize * 10) / 10,
    letterSpacing: Math.round(letterSpacing * 10) / 10,
    x: RANK_STROKE / 2,
    y: Math.round((RANK_H - RANK_STROKE / 2 - RANK_INK_BOTTOM * fontSize) * 10) / 10,
  };
}

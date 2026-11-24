/**
 * Vertical swipe to change channel on a phone (#291). Pure: the caller feeds it touch points and
 * the clock, so it can be unit-tested and works the same with Touch or Pointer Events.
 *
 * A swipe has to be clearly vertical and long enough, so taps and small drags still reach the
 * tap toggle, and horizontal drags are left alone. A second finger cancels the gesture.
 */

/** Minimum vertical travel, in CSS px. A tap wobbles a few px; a deliberate swipe is much longer. */
export const SWIPE_MIN = 60;
/** The vertical travel must be at least this many times the horizontal travel. */
export const SWIPE_RATIO = 2;
/** Longer than this and it's a slow drag, not a swipe. */
export const SWIPE_MAX_MS = 800;
/** Horizontal travel past this, while it's still more sideways than vertical, cancels the gesture. */
export const SWIPE_SLOP = 12;

/** +1 next channel (swipe up), −1 previous channel (swipe down), 0 not a swipe. */
export type SwipeDir = 1 | -1 | 0;

export interface SwipeOpts {
  min: number;
  ratio: number;
  maxMs: number;
  slop: number;
}
const DEFAULTS: SwipeOpts = { min: SWIPE_MIN, ratio: SWIPE_RATIO, maxMs: SWIPE_MAX_MS, slop: SWIPE_SLOP };

/**
 * Classify a finished one-finger gesture. Screen y grows downwards, so dy < 0 is a swipe up,
 * which matches Channel Up / ▲ (next channel).
 */
export function classifySwipe(dx: number, dy: number, ms: number, opts: SwipeOpts = DEFAULTS): SwipeDir {
  if (ms > opts.maxMs) return 0;
  const ay = Math.abs(dy);
  if (ay < opts.min || ay < Math.abs(dx) * opts.ratio) return 0;
  return dy < 0 ? 1 : -1;
}

/** The bits of a Touch (or PointerEvent) the tracker reads. */
export interface SwipePoint {
  identifier: number;
  clientX: number;
  clientY: number;
}

/**
 * Tracks one gesture from start to end. Pass the event's `touches` (all fingers now down) to
 * `start`/`move`, and its `changedTouches` plus `touches.length` to `end`.
 */
export function createSwipe(opts: SwipeOpts = DEFAULTS) {
  let s: { id: number; x: number; y: number; t: number } | null = null;
  const find = (pts: ArrayLike<SwipePoint>) => {
    for (let i = 0; i < pts.length; i++) if (pts[i]!.identifier === s!.id) return pts[i]!;
    return undefined;
  };
  return {
    start(touches: ArrayLike<SwipePoint>, now: number): void {
      // A second finger cancels: pinch, two-finger gestures and palm touches aren't swipes.
      if (touches.length !== 1) {
        s = null;
        return;
      }
      const p = touches[0]!;
      s = { id: p.identifier, x: p.clientX, y: p.clientY, t: now };
    },
    move(touches: ArrayLike<SwipePoint>): void {
      if (!s) return;
      if (touches.length !== 1) {
        s = null;
        return;
      }
      const p = find(touches);
      if (!p) return;
      const dx = Math.abs(p.clientX - s.x);
      // Went sideways first (a seek or volume drag): it stays not-a-swipe even if it turns.
      if (dx > opts.slop && dx > Math.abs(p.clientY - s.y)) s = null;
    },
    end(changed: ArrayLike<SwipePoint>, remaining: number, now: number): SwipeDir {
      if (!s) return 0;
      const p = find(changed);
      if (!p) return 0;
      const g = s;
      s = null;
      if (remaining > 0) return 0;
      return classifySwipe(p.clientX - g.x, p.clientY - g.y, now - g.t, opts);
    },
    /** touchcancel, or the swipe became disabled mid-gesture. */
    cancel(): void {
      s = null;
    },
    /** Whether a gesture is being tracked. */
    active: () => s !== null,
  };
}

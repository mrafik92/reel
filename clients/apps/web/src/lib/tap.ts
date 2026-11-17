import { createEffect, on, onCleanup } from "solid-js";

/** How long a click or tap waits to see whether it becomes a double. Also the double-tap window. */
export const SINGLE_CLICK_DELAY = 300;
/** Controls that came up this recently (a mouse move onto the picture, a tap) count as hidden. */
export const JUST_SHOWN = 600;

/**
 * Click handling for a video surface where a single click shows/hides the controls and a
 * double-click does something else (full screen). The single-click action runs only once
 * we know no second click is coming, so a double-click doesn't flicker the controls.
 *
 * `shown` is sampled at pointerdown, and controls that appeared in the last JUST_SHOWN ms
 * count as hidden: moving the mouse onto the picture (or the compatibility mousemove a
 * touch tap fires) already brings the controls up, and the click that follows should keep
 * them up rather than hide them again. `shown` must be reactive.
 */
export function createTapToggle(opts: { shown: () => boolean; show: () => void; hide: () => void }) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let sampled: boolean | undefined;
  let since = 0;
  createEffect(on(opts.shown, (v) => v && (since = Date.now())));
  const settled = () => opts.shown() && Date.now() - since >= JUST_SHOWN;
  const cancel = () => {
    if (timer) clearTimeout(timer);
    timer = undefined;
  };
  onCleanup(cancel);
  return {
    onPointerDown: () => {
      sampled = settled();
    },
    onClick: (ev: MouseEvent) => {
      cancel();
      const hide = sampled ?? settled();
      sampled = undefined;
      // detail ≥ 2 is the second click of a double-click: the dblclick handler owns it.
      if (ev.detail > 1) return;
      const run = () => {
        timer = undefined;
        if (hide) opts.hide();
        else opts.show();
      };
      // detail 0 is a keyboard/remote activation (OK on a focused surface): no double-click to wait for.
      if (ev.detail === 0) run();
      else timer = setTimeout(run, SINGLE_CLICK_DELAY);
    },
    /** Call from onDblClick before acting, in case the browser reports detail = 1 twice. */
    cancel,
  };
}

/**
 * How a pointer should act on the picture (#183, YouTube's split): a mouse click plays/pauses
 * at once, a touch or pen tap shows/hides the controls. A click with no pointer type (keyboard
 * activation, a browser without Pointer Events) takes the safe touch path: it never pauses.
 */
export const pointerKind = (type: string | undefined): "mouse" | "touch" => (type === "mouse" ? "mouse" : "touch");

/** Double-tap seek: each step, and how long after a seek tap another tap on that side adds to it. */
export const SEEK_STEP = 10;
export const SEEK_STREAK = 800;

/** Which third of the picture a tap landed in: −1 left, 0 centre, 1 right. */
export type Zone = -1 | 0 | 1;
export const zoneOf = (x: number, width: number): Zone => (x < width / 3 ? -1 : x > (2 * width) / 3 ? 1 : 0);

export type TapResult =
  /** Maybe a single tap: run the single-tap action once SINGLE_CLICK_DELAY passes without another. */
  | { kind: "single" }
  /** A double-tap in the centre third. */
  | { kind: "double" }
  /** Seek by `delta` now; `total` is the streak so far (for the "−30 s" ripple). */
  | { kind: "seek"; delta: number; total: number; zone: -1 | 1 };

/**
 * YouTube's double-tap seek for touch. A double-tap on the left or right third seeks ∓SEEK_STEP;
 * while the streak lasts (SEEK_STREAK after the last seek tap) every further tap on that side
 * adds another step, so taps add up. A tap on the other side starts a new streak that way;
 * a centre tap ends the streak and counts as a fresh first tap. Pure: the caller passes the clock.
 */
export function createDoubleTap() {
  let last = -Infinity;
  let streakZone: Zone = 0;
  let streakUntil = -Infinity;
  let total = 0;
  const seek = (now: number, zone: -1 | 1, fresh: boolean): TapResult => {
    total = (fresh ? 0 : total) + zone * SEEK_STEP;
    streakZone = zone;
    streakUntil = now + SEEK_STREAK;
    last = -Infinity;
    return { kind: "seek", delta: zone * SEEK_STEP, total, zone };
  };
  return {
    tap(now: number, zone: Zone): TapResult {
      if (zone !== 0 && now <= streakUntil) return seek(now, zone, zone !== streakZone);
      streakUntil = -Infinity;
      if (now - last <= SINGLE_CLICK_DELAY) {
        last = -Infinity;
        return zone === 0 ? { kind: "double" } : seek(now, zone, true);
      }
      last = now;
      return { kind: "single" };
    },
  };
}

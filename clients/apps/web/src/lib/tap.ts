import { createEffect, on, onCleanup } from "solid-js";

/** How long a click waits to see whether it becomes a double-click. */
export const SINGLE_CLICK_DELAY = 250;
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

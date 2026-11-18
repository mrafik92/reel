import { createContext } from "solid-js";

/**
 * Full screen for the player (#247).
 *
 * "Next episode" swaps the item under the same `/play/:id` route, and the keyed `<Show>` builds
 * a fresh `PlayerView`. If the player's own stage were the full-screen element, removing it
 * would drop full screen (and turn the phone back to portrait in Reel Web for Android, which
 * follows the page). So the route wrapper (`div.player`, which stays mounted while only the id
 * changes) is the full-screen element, and the wrapper, not the view, leaves full screen when the
 * player route closes (Back, ✕, another page). A view without a wrapper falls back to its stage.
 */

type FsDoc = {
  fullscreenElement?: Element | null;
  exitFullscreen?: () => Promise<void>;
};
type FsEl = { requestFullscreen?: () => Promise<void> };

/** The element that stays mounted across an episode change; provided by the player routes. */
export const FullscreenHost = createContext<() => HTMLElement | undefined>();

/** Leave full screen if anything is full screen. */
export function leaveFullscreen(doc: FsDoc): void {
  if (doc.fullscreenElement) doc.exitFullscreen?.().catch(() => undefined);
}

/**
 * Full-screen control for one player view. `toggle` puts the host (else the view's own stage) in
 * full screen; `release` runs when the view goes away and only leaves full screen when there's
 * no host to keep it (the host's route does that on its own cleanup).
 */
export function playerFullscreen<E extends FsEl>(o: {
  doc: FsDoc;
  host: (() => E | undefined) | undefined;
  own: () => E | undefined;
}): { target: () => E | undefined; toggle: () => void; release: () => void } {
  const target = () => o.host?.() ?? o.own();
  return {
    target,
    toggle: () => {
      if (o.doc.fullscreenElement) return leaveFullscreen(o.doc);
      target()?.requestFullscreen?.().catch(() => undefined);
    },
    release: () => {
      if (!o.host) leaveFullscreen(o.doc);
    },
  };
}

import { createEffect, onCleanup, onMount } from "solid-js";
import { useLocation } from "@solidjs/router";
import { focusById, focusFirst, isTvMode } from "@reel/ui";

interface Saved {
  fid?: string;
  y: number;
}
const saved = new Map<string, Saved>();

export interface PageFocusOptions {
  /**
   * With nothing saved to return to, focus the page's first control (`[data-autofocus]`, else the
   * first focusable). Default on. Search turns it off on touch and desktop when it opens on a query:
   * its first control is the search box, and focusing that opens the phone's keyboard (#227).
   */
  focusFirst?: () => boolean;
}

/**
 * Remember focus + scroll for a page and restore them once the page has data
 * (`ready()`), so Back on a remote, or in a browser or Reel Web for Android, lands exactly where the
 * viewer left off.
 */
export function usePageFocus(ready: () => boolean, container?: () => HTMLElement | undefined, opts: PageFocusOptions = {}): void {
  const loc = useLocation();
  const path = loc.pathname;
  let key = path + loc.search;
  // A page may rewrite its own query while it's open (Search keeps `?q=` in step with the box,
  // `replace: true`): save under the address it has when it's left, which is where Back returns.
  createEffect(() => {
    if (loc.pathname === path) key = path + loc.search;
  });
  let done = false;
  let fid: string | undefined;
  let y = 0;

  onMount(() => {
    const onFocus = (e: FocusEvent) => {
      const holder = (e.target as HTMLElement | null)?.closest?.<HTMLElement>("[data-fid]");
      fid = holder?.getAttribute("data-fid") ?? undefined;
    };
    const onScroll = () => {
      if (done) y = window.pageYOffset;
    };
    document.addEventListener("focusin", onFocus);
    window.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => {
      document.removeEventListener("focusin", onFocus);
      window.removeEventListener("scroll", onScroll);
      saved.set(key, { fid, y });
    });
  });

  createEffect(() => {
    let ok: boolean;
    try {
      ok = ready();
    } catch {
      // Reading an errored resource throws; the page is showing its error state.
      ok = true;
    }
    if (done || !ok) return;
    // Let the DOM settle (rows mount, grid measures) before measuring.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        done = true;
        const s = saved.get(key);
        window.scrollTo(0, s ? s.y : 0);
        if (s?.fid && focusById(s.fid)) {
          // Focusing brings the card into view in its row. On TV it also re-anchors the page; with
          // touch or a mouse the page goes back to exactly where it was.
          if (!isTvMode()) window.scrollTo(0, s.y);
          return;
        }
        if (opts.focusFirst && !opts.focusFirst()) return;
        const root = container?.() ?? document.querySelector("main") ?? document.body;
        focusFirst(root);
      }),
    );
  });
}

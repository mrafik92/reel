import { createEffect, onCleanup, onMount } from "solid-js";
import { useLocation } from "@solidjs/router";
import { focusById, focusFirst } from "@reel/ui";

interface Saved {
  fid?: string;
  y: number;
}
const saved = new Map<string, Saved>();

/**
 * Remember focus + scroll for a page and restore them once the page has data
 * (`ready()`), so Back on a remote lands exactly where the viewer left off.
 */
export function usePageFocus(ready: () => boolean, container?: () => HTMLElement | undefined): void {
  const loc = useLocation();
  const key = loc.pathname + loc.search;
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
    if (done || !ready()) return;
    // Let the DOM settle (rows mount, grid measures) before measuring.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        done = true;
        const s = saved.get(key);
        window.scrollTo(0, s ? s.y : 0);
        if (s?.fid && focusById(s.fid)) return;
        const root = container?.() ?? document.querySelector("main") ?? document.body;
        focusFirst(root);
      }),
    );
  });
}

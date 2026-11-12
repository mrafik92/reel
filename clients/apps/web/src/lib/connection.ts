/**
 * "The app is back": the tab became visible again, the browser came back online, or the page was
 * restored from the back/forward cache (#81). Whatever went stale while it was away (a failed
 * request, a dead event socket, a boot that couldn't reach the server) subscribes here and recovers
 * by itself, so a page refresh is never needed.
 */

export interface ResumeInfo {
  /** How long the page was hidden (0 for `online`). */
  awayMs: number;
  reason: "visible" | "online" | "pageshow";
}

type Handler = (r: ResumeInfo) => void;
const handlers = new Set<Handler>();
let hiddenAt = 0;

export function onResume(fn: Handler): () => void {
  handlers.add(fn);
  return () => handlers.delete(fn);
}

function fire(reason: ResumeInfo["reason"]): void {
  const awayMs = hiddenAt ? Date.now() - hiddenAt : 0;
  hiddenAt = 0;
  const r = { awayMs, reason };
  handlers.forEach((h) => {
    try {
      h(r);
    } catch {
      /* one bad subscriber must not block the others */
    }
  });
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) hiddenAt = Date.now();
    else fire("visible");
  });
  window.addEventListener("online", () => fire("online"));
  window.addEventListener("pageshow", (e) => {
    if ((e as PageTransitionEvent).persisted) fire("pageshow");
  });
}

/**
 * Retry `fn` with backoff while `active()` holds, and right away on resume. Returns a stop function.
 * Used for "Can't reach your Reel server" states.
 */
export function autoRetry(fn: () => void, active: () => boolean, delays = [3000, 6000, 12000, 20000, 30000]): () => void {
  let i = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  const schedule = () => {
    if (stopped) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      if (stopped || !active()) return;
      // Hidden tabs wait for `visible` instead of polling.
      if (typeof document !== "undefined" && document.hidden) return;
      i = Math.min(i + 1, delays.length - 1);
      fn();
      schedule();
    }, delays[i]);
  };
  const off = onResume(() => {
    if (stopped || !active()) return;
    i = 0;
    fn();
    schedule();
  });
  schedule();
  return () => {
    stopped = true;
    off();
    if (timer) clearTimeout(timer);
  };
}

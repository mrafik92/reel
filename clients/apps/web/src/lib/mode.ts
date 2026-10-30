import { isTvUserAgent } from "@reel/ui";

function flag(name: string, envOn: boolean): boolean {
  const key = `reel.${name}`;
  let q: string | null = null;
  try {
    q = new URLSearchParams(location.search).get(name);
  } catch {
    /* ignore */
  }
  try {
    if (q === "1" || q === "true") sessionStorage.setItem(key, "1");
    else if (q === "0" || q === "false") sessionStorage.removeItem(key);
    if (sessionStorage.getItem(key) === "1") return true;
  } catch {
    /* storage disabled */
  }
  return q === "1" || q === "true" || (q == null && envOn);
}

/** Mock data mode: `?mock=1` (sticky for the session) or `VITE_MOCK=1`. `?mock=0` turns it off. */
export const MOCK = flag("mock", import.meta.env.VITE_MOCK === "1");

/** 10-foot mode: TV user agents, `VITE_TARGET=tv`, or `?tv=1` for previewing in a desktop browser. */
export const TV = flag("tv", import.meta.env.VITE_TARGET === "tv") || isTvUserAgent();

/** Packaged TV apps load from file:// — hash routing works there without a server fallback. */
export const HASH_ROUTING = typeof location !== "undefined" && location.protocol === "file:";

/** Build an in-app href that works in both history and hash routing. */
export function href(path: string): string {
  return HASH_ROUTING ? "#" + path : path;
}

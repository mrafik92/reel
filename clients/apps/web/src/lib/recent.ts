import { createSignal } from "solid-js";

const KEY = "reel.recentSearches";
const MAX = 8;

function load(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) || "[]") as unknown;
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, MAX) : [];
  } catch {
    return [];
  }
}

const [recent, setRecent] = createSignal<string[]>(load());

function save(list: string[]): void {
  setRecent(list);
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    /* storage disabled */
  }
}

/** Most recent first, case-insensitively de-duplicated. */
export const recentSearches = recent;

export function rememberSearch(q: string): void {
  const t = q.trim();
  if (t.length < 2) return;
  save([t].concat(recent().filter((x) => x.toLowerCase() !== t.toLowerCase())).slice(0, MAX));
}

export function forgetRecent(q: string): void {
  save(recent().filter((x) => x !== q));
}

export function clearRecent(): void {
  save([]);
}

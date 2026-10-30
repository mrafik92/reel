import type { Home, ItemDetail, ItemSummary, Library, LibrarySort, Page, Id } from "@reel/ui";
import { setApiBase } from "@reel/ui";
import { API_BASE, MOCK } from "../lib/mode";

export type { Home, ItemDetail, ItemSummary, Library, LibrarySort, Page, Id };

export interface ProgressReport {
  item_id: Id;
  file_id: Id;
  position_s: number;
  duration_s: number;
  paused: boolean;
  /** "tick" every ~10s while playing, otherwise the triggering event. */
  reason: "tick" | "pause" | "ended" | "stop";
}

/** Phase 1 data access (SPEC §4.1). Implemented over fetch, or fixtures in mock mode. */
export interface Api {
  readonly mock: boolean;
  libraries(): Promise<Library[]>;
  libraryItems(id: Id, opts: { sort: LibrarySort; cursor?: string | null; limit?: number }): Promise<Page<ItemSummary>>;
  item(id: Id): Promise<ItemDetail>;
  children(id: Id): Promise<ItemSummary[]>;
  home(): Promise<Home>;
  /** Movies + shows only (no episodes). */
  search(q: string, limit?: number): Promise<ItemSummary[]>;
  /**
   * STUB: Phase 1 has no playback session, so there is nowhere to send this yet.
   * Phase 2: `POST /playback/{sessionId}/progress` after `/playback/{fileId}/decide`.
   */
  reportProgress(r: ProgressReport): Promise<void>;
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

setApiBase(API_BASE);

function token(): string | null {
  try {
    return localStorage.getItem("reel.token");
  } catch {
    return null;
  }
}

async function get<T>(path: string, params?: Record<string, string | number | null | undefined>): Promise<T> {
  let qs = "";
  if (params) {
    const p = new URLSearchParams();
    for (const k in params) {
      const v = params[k];
      if (v != null && v !== "") p.set(k, String(v));
    }
    const s = p.toString();
    if (s) qs = "?" + s;
  }
  const headers: Record<string, string> = { Accept: "application/json" };
  const t = token();
  if (t) headers.Authorization = `Bearer ${t}`;
  let res: Response;
  try {
    res = await fetch(`${API_BASE}/api/v1${path}${qs}`, { headers });
  } catch {
    throw new ApiError(0, "Can't reach your Reel server");
  }
  if (!res.ok) throw new ApiError(res.status, res.status === 404 ? "Not found" : `Server error (${res.status})`);
  return (await res.json()) as T;
}

const httpApi: Api = {
  mock: false,
  libraries: () => get<Library[]>("/libraries"),
  libraryItems: (id, o) =>
    get<Page<ItemSummary>>(`/libraries/${id}/items`, { sort: o.sort, cursor: o.cursor, limit: o.limit ?? 60 }),
  item: (id) => get<ItemDetail>(`/items/${id}`),
  // `/children` and `/search` return plain ItemSummary[]; only library items are paged.
  children: (id) => get<ItemSummary[]>(`/items/${id}/children`),
  home: () => get<Home>("/home"),
  search: (q, limit = 60) => get<ItemSummary[]>("/search", { q, limit }),
  reportProgress: async (r) => {
    if (import.meta.env.DEV) console.debug("[reel] progress (stub)", r);
  },
};

let impl: Promise<Api> | undefined;
function load(): Promise<Api> {
  if (!impl) impl = MOCK ? import("./mock").then((m) => m.createMockApi()) : Promise.resolve(httpApi);
  return impl;
}

// Short-lived cache so Back navigation renders instantly.
const cache = new Map<string, { at: number; p: Promise<unknown> }>();
const TTL = 30_000;
function cached<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const hit = cache.get(key);
  if (hit && Date.now() - hit.at < TTL) return hit.p as Promise<T>;
  const p = fn();
  cache.set(key, { at: Date.now(), p });
  p.catch(() => cache.delete(key));
  return p;
}
export function invalidate(prefix = ""): void {
  cache.forEach((_v, k) => {
    if (k.indexOf(prefix) === 0) cache.delete(k);
  });
}

export const api: Api = {
  mock: MOCK,
  libraries: () => cached("libraries", () => load().then((a) => a.libraries())),
  libraryItems: (id, o) =>
    cached(`lib:${id}:${o.sort}:${o.cursor ?? ""}:${o.limit ?? ""}`, () => load().then((a) => a.libraryItems(id, o))),
  item: (id) => cached(`item:${id}`, () => load().then((a) => a.item(id))),
  children: (id) => cached(`children:${id}`, () => load().then((a) => a.children(id))),
  home: () => cached("home", () => load().then((a) => a.home())),
  search: (q, limit = 60) => cached(`search:${q}:${limit}`, () => load().then((a) => a.search(q, limit))),
  reportProgress: (r) =>
    load().then((a) => {
      invalidate("home");
      invalidate(`item:${r.item_id}`);
      return a.reportProgress(r);
    }),
};

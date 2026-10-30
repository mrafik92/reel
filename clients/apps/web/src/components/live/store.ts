/**
 * Live TV state shared by the channel list, the player page and the guide.
 *
 * Built for big providers (10k+ channels, 150+ groups): the first paint only needs the group list,
 * the favourites and the current group (small requests); the full list loads in the background and
 * then powers instant search, number entry and "All channels". Now/next is fetched lazily for
 * whatever is on screen.
 */
import { createMemo, createRoot, createSignal } from "solid-js";
import { iptv, type IptvChannel, type IptvGroup, type NowNext, type Program } from "../../api/iptv";

export type Filter = { kind: "all" } | { kind: "favorites" } | { kind: "group"; name: string };

const FILTER_KEY = "reel.live.filter";
const RECENT_KEY = "reel.live.recentGroups";
const LAST_KEY = "reel.live.last";

function readJson<T>(k: string): T | undefined {
  try {
    const v = localStorage.getItem(k);
    return v ? (JSON.parse(v) as T) : undefined;
  } catch {
    return undefined;
  }
}
function writeJson(k: string, v: unknown): void {
  try {
    localStorage.setItem(k, JSON.stringify(v));
  } catch {
    /* ignore */
  }
}
function validFilter(f: unknown): f is Filter {
  const x = f as Filter | undefined;
  return !!x && (x.kind === "all" || x.kind === "favorites" || (x.kind === "group" && typeof (x as { name?: unknown }).name === "string"));
}

export const nowSec = () => Math.floor(Date.now() / 1000);

export type LoadState = "loading" | "error" | "empty" | "ready";

const store = createRoot(() => {
  const [state, setState] = createSignal<LoadState>("loading");
  const [error, setError] = createSignal<unknown>();
  const [groups, setGroups] = createSignal<IptvGroup[]>([]);
  const [favs, setFavs] = createSignal<IptvChannel[] | undefined>();
  const [all, setAll] = createSignal<IptvChannel[] | undefined>();
  const [byGroup, setByGroup] = createSignal<Record<string, IptvChannel[]>>({});
  const [filter, setFilterSig] = createSignal<Filter>({ kind: "favorites" });
  const [query, setQuery] = createSignal("");
  const [remote, setRemote] = createSignal<{ q: string; list: IptvChannel[] } | undefined>();
  const [recent, setRecent] = createSignal<string[]>(readJson<string[]>(RECENT_KEY) ?? []);
  const [clock, setClock] = createSignal(nowSec());
  const [nowRev, setNowRev] = createSignal(0);
  const [knownRev, setKnownRev] = createSignal(0);
  setInterval(() => setClock(nowSec()), 15_000);

  /** Group lists derived from the full list once it's there (instant switching). */
  const groupIndex = createMemo(() => {
    const a = all();
    if (!a) return undefined;
    const m: Record<string, IptvChannel[]> = {};
    for (const c of a) {
      const g = c.group ?? "";
      (m[g] || (m[g] = [])).push(c);
    }
    return m;
  });

  return {
    state, setState, error, setError, groups, setGroups, favs, setFavs, all, setAll, byGroup, setByGroup, filter, setFilterSig, query, setQuery,
    remote, setRemote, recent, setRecent, clock, setClock, nowRev, setNowRev, knownRev, setKnownRev, groupIndex,
  };
});

export const loadState = store.state;
export const channelsError = store.error;
export const groups = store.groups;
export const filter = store.filter;
export const query = store.query;
export const recentGroups = store.recent;
/** Ticks every 15 s — drives progress bars and the guide's now line. */
export const clock = store.clock;
/** The full channel list (undefined until the background load finishes). */
export const allChannels = store.all;
export const favourites = store.favs;

export const totalChannels = () => store.groups().reduce((n, g) => n + g.count, 0);
export const favouriteCount = () => (store.favs() ?? []).length;

// ---- known channels (anything we've seen, for lookups by id) -----------------------

const known = new Map<number, IptvChannel>();
function remember(list: IptvChannel[]): void {
  for (const c of list) known.set(c.id, c);
  store.setKnownRev((v) => v + 1);
}

export function channelById(id: number | undefined): IptvChannel | undefined {
  if (id == null) return undefined;
  store.knownRev();
  return known.get(id);
}

/** By channel number — needs the full list; falls back to what's loaded so far. */
export function channelByNumber(n: number): IptvChannel | undefined {
  store.knownRev();
  const a = store.all();
  if (a) return a.find((c) => c.number === n);
  for (const c of known.values()) if (c.number === n) return c;
  return undefined;
}

// ---- loading ------------------------------------------------------------------------

let loading: Promise<void> | undefined;
let loadedAt = 0;
let allLoading: Promise<IptvChannel[]> | undefined;

export function filterKey(f: Filter): string {
  return f.kind === "group" ? `g:${f.name}` : f.kind;
}

/** Groups + favourites first (small); pick the starting filter; then the full list in the background. */
export function loadChannels(force = false): Promise<void> {
  if (loading) return loading;
  if (!force && store.state() === "ready" && Date.now() - loadedAt < 5 * 60_000) return Promise.resolve();
  if (store.state() !== "ready") store.setState("loading");
  store.setError(undefined);
  loading = Promise.all([iptv.groups(), iptv.channels({ favorites: true })])
    .then(([g, f]) => {
      loadedAt = Date.now();
      g = g.slice().sort((a, b) => a.name.localeCompare(b.name));
      store.setGroups(g);
      store.setFavs(f);
      remember(f);
      const last = readJson<IptvChannel>(LAST_KEY);
      if (last && last.id) known.has(last.id) || remember([last]);
      if (!g.length) {
        store.setState("empty");
        return;
      }
      // Starting view: the saved one if it still exists, else favourites, else the first group.
      const saved = readJson<Filter>(FILTER_KEY);
      let start: Filter =
        validFilter(saved) && (saved.kind !== "group" || g.some((x) => x.name === saved.name)) && (saved.kind !== "favorites" || f.length > 0)
          ? saved
          : f.length
            ? { kind: "favorites" }
            : { kind: "group", name: g[0]!.name };
      if (force && store.state() === "ready") start = store.filter();
      store.setFilterSig(start);
      void ensureList(start);
      store.setState("ready");
      if (force) {
        store.setAll(undefined);
        store.setByGroup({});
        allLoading = undefined;
      }
      // Full list for search / number entry / "All channels" — big, so after first paint.
      setTimeout(() => void loadAll(), 1200);
    })
    .catch((e) => {
      store.setError(e);
      store.setState("error");
    })
    .finally(() => {
      loading = undefined;
    });
  return loading;
}

/** Load (once) the full channel list. */
export function loadAll(): Promise<IptvChannel[]> {
  const a = store.all();
  if (a) return Promise.resolve(a);
  if (!allLoading) {
    allLoading = iptv
      .channels()
      .then((l) => {
        remember(l);
        store.setAll(l);
        return l;
      })
      .catch((e) => {
        allLoading = undefined;
        throw e;
      });
  }
  return allLoading;
}

const groupLoads = new Map<string, Promise<void>>();
function ensureList(f: Filter): Promise<void> {
  if (f.kind === "all") return loadAll().then(() => undefined, () => undefined);
  if (f.kind !== "group" || store.groupIndex() || store.byGroup()[f.name]) return Promise.resolve();
  let p = groupLoads.get(f.name);
  if (!p) {
    p = iptv
      .channels({ group: f.name })
      .then((l) => {
        remember(l);
        store.setByGroup((m) => ({ ...m, [f.name]: l }));
      })
      .catch(() => undefined)
      .finally(() => groupLoads.delete(f.name));
    groupLoads.set(f.name, p);
  }
  return p;
}

export function setFilter(f: Filter): void {
  store.setFilterSig(f);
  writeJson(FILTER_KEY, f);
  if (f.kind === "group") {
    const r = [f.name].concat(store.recent().filter((x) => x !== f.name)).slice(0, 8);
    store.setRecent(r);
    writeJson(RECENT_KEY, r);
  }
  void ensureList(f);
}

export function filterLabel(f: Filter = store.filter()): string {
  return f.kind === "all" ? "All channels" : f.kind === "favorites" ? "Favourites" : f.name;
}

// ---- search -----------------------------------------------------------------------------

const fold = (s: string) => s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");
let searchTimer: ReturnType<typeof setTimeout> | undefined;

/** Instant over the full list once loaded; until then the server's `q=` (debounced). */
export function setQuery(q: string): void {
  store.setQuery(q);
  if (searchTimer) clearTimeout(searchTimer);
  const t = q.trim();
  if (!t || store.all() || /^\d+$/.test(t)) return;
  searchTimer = setTimeout(() => {
    iptv
      .channels({ q: t })
      .then((l) => {
        remember(l);
        if (store.query().trim() === t) store.setRemote({ q: t, list: l });
      })
      .catch(() => undefined);
  }, 220);
}

const searchMemo = createRoot(() =>
  createMemo((): IptvChannel[] | undefined => {
    const t = fold(store.query().trim());
    if (!t) return undefined;
    const a = store.all();
    const num = /^\d+$/.test(t) ? Number(t) : null;
    if (a) {
      const out: IptvChannel[] = [];
      if (num != null) {
        // Number first, then names containing the digits.
        for (const c of a) if (c.number === num) out.push(c);
      }
      for (const c of a) if (fold(c.name).indexOf(t) >= 0 && c.number !== num) out.push(c);
      return out;
    }
    if (num != null) {
      store.knownRev();
      const c = channelByNumber(num);
      return c ? [c] : [];
    }
    const r = store.remote();
    return r && fold(r.q) === t ? r.list : undefined;
  }),
);

/** The list for the current search or filter; `undefined` while it loads. */
export function current(): IptvChannel[] | undefined {
  const s = searchMemo();
  if (store.query().trim()) return s;
  return listFor(store.filter());
}

/** The current filter's list (ignoring search); `undefined` while it loads. */
export function filteredList(): IptvChannel[] | undefined {
  return listFor(store.filter());
}

/** The current filter's list, ignoring search. */
export function filtered(): IptvChannel[] {
  return listFor(store.filter()) ?? [];
}

function listFor(f: Filter): IptvChannel[] | undefined {
  if (f.kind === "favorites") return store.favs();
  if (f.kind === "all") return store.all();
  const gi = store.groupIndex();
  if (gi) return gi[f.name] ?? [];
  return store.byGroup()[f.name];
}

// ---- favourites ---------------------------------------------------------------------------

function replaceEverywhere(c: IptvChannel): void {
  known.set(c.id, c);
  const sub = (l: IptvChannel[] | undefined) => (l && l.some((x) => x.id === c.id) ? l.map((x) => (x.id === c.id ? c : x)) : l);
  store.setAll(sub(store.all()));
  const bg = store.byGroup();
  if (c.group && bg[c.group]) store.setByGroup({ ...bg, [c.group]: sub(bg[c.group])! });
  const f = store.favs() ?? [];
  store.setFavs(c.favorite ? (f.some((x) => x.id === c.id) ? sub(f) : f.concat(c).sort((a, b) => (a.number ?? 1e9) - (b.number ?? 1e9))) : f.filter((x) => x.id !== c.id));
  store.setKnownRev((v) => v + 1);
}

export async function toggleFavorite(c: IptvChannel): Promise<void> {
  const on = !c.favorite;
  replaceEverywhere({ ...c, favorite: on });
  try {
    await iptv.favorite(c.id, on);
  } catch (e) {
    replaceEverywhere({ ...c, favorite: !on });
    throw e;
  }
}

// ---- now / next ---------------------------------------------------------------------------

const nowMap = new Map<number, NowNext & { at: number }>();
const inflight = new Set<number>();
let queue: number[] = [];
let flushTimer: ReturnType<typeof setTimeout> | undefined;

/** Now/next for a channel (reactive; `undefined` until fetched). */
export function nowNext(id: number): (NowNext & { at: number }) | undefined {
  store.nowRev();
  const n = nowMap.get(id);
  if (!n) return undefined;
  // Roll forward locally when the current programme ended and we know the next.
  const t = store.clock();
  if (n.now && n.now.stop <= t && n.next && n.next.stop > t) return { ...n, now: n.next, next: null };
  return n;
}

function stale(id: number, t: number): boolean {
  const n = nowMap.get(id);
  if (!n) return true;
  if (t - n.at > 15 * 60) return true;
  // Programme over and we don't know what follows: refetch.
  return !!n.now && n.now.stop <= t && (!n.next || n.next.stop <= t);
}

/** Make sure now/next is loaded for these channels (batched, de-duplicated). */
export function ensureNow(ids: number[]): void {
  const t = nowSec();
  ids.forEach((id) => {
    if (!inflight.has(id) && stale(id, t) && queue.indexOf(id) < 0) queue.push(id);
  });
  if (!queue.length || flushTimer) return;
  flushTimer = setTimeout(flush, 60);
}

function flush(): void {
  flushTimer = undefined;
  const batch = queue.splice(0, 100);
  if (!batch.length) return;
  batch.forEach((id) => inflight.add(id));
  iptv
    .now(batch)
    .then((list) => {
      const at = nowSec();
      list.forEach((n) => nowMap.set(n.channel_id, { ...n, at }));
      // Channels the server didn't answer for: remember "nothing" so we don't hammer it.
      batch.forEach((id) => !nowMap.has(id) && nowMap.set(id, { channel_id: id, now: null, next: null, at }));
      store.setNowRev((v) => v + 1);
    })
    .catch(() => undefined)
    .finally(() => {
      batch.forEach((id) => inflight.delete(id));
      if (queue.length) flushTimer = setTimeout(flush, 30);
    });
}

export function progressOf(p: Pick<Program, "start" | "stop"> | null | undefined, t: number): number {
  if (!p) return 0;
  return Math.max(0, Math.min(1, (t - p.start) / Math.max(1, p.stop - p.start)));
}

// ---- last channel ---------------------------------------------------------------------------

export function lastChannel(): IptvChannel | undefined {
  const c = readJson<IptvChannel>(LAST_KEY);
  return c && typeof c.id === "number" ? known.get(c.id) ?? c : undefined;
}
export function rememberChannel(c: IptvChannel): void {
  writeJson(LAST_KEY, c);
}

// ---- formatting -----------------------------------------------------------------------------

const tf = (() => {
  try {
    return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  } catch {
    return undefined;
  }
})();

export function fmtTime(unix: number): string {
  const d = new Date(unix * 1000);
  if (tf) return tf.format(d);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function fmtRange(p: Program): string {
  return `${fmtTime(p.start)} – ${fmtTime(p.stop)}`;
}

export function minutesLeft(p: Program, t: number): string {
  const m = Math.max(0, Math.round((p.stop - t) / 60));
  if (m >= 60) return `${Math.floor(m / 60)} h ${m % 60 ? `${m % 60} min ` : ""}left`;
  return `${m} min left`;
}

export function fmtDay(unix: number): string {
  const d = new Date(unix * 1000);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diff = Math.round((new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() - today.getTime()) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Tomorrow";
  if (diff === -1) return "Yesterday";
  try {
    return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "short" });
  } catch {
    return d.toDateString();
  }
}

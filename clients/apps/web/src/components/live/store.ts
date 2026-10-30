/**
 * Live TV state shared by the channel list, the player page and the guide:
 * channel list, groups, filter, now/next cache (fetched lazily for what's on screen) and a clock.
 */
import { createRoot, createSignal } from "solid-js";
import { iptv, type IptvChannel, type IptvGroup, type NowNext, type Program } from "../../api/iptv";

export type Filter = { kind: "all" } | { kind: "favorites" } | { kind: "group"; name: string };

const FILTER_KEY = "reel.live.filter";
const LAST_KEY = "reel.live.last";

function loadFilter(): Filter {
  try {
    const f = JSON.parse(sessionStorage.getItem(FILTER_KEY) || "null") as Filter | null;
    if (f && (f.kind === "all" || f.kind === "favorites" || (f.kind === "group" && typeof f.name === "string"))) return f;
  } catch {
    /* ignore */
  }
  return { kind: "all" };
}

export const nowSec = () => Math.floor(Date.now() / 1000);

const store = createRoot(() => {
  const [channels, setChannels] = createSignal<IptvChannel[] | undefined>(undefined);
  const [groups, setGroups] = createSignal<IptvGroup[]>([]);
  const [error, setError] = createSignal<unknown>();
  const [filter, setFilterSig] = createSignal<Filter>(loadFilter());
  const [clock, setClock] = createSignal(nowSec());
  const [nowRev, setNowRev] = createSignal(0);
  setInterval(() => setClock(nowSec()), 15_000);
  return { channels, setChannels, groups, setGroups, error, setError, filter, setFilterSig, clock, nowRev, setNowRev };
});

export const channels = store.channels;
export const groups = store.groups;
export const channelsError = store.error;
export const filter = store.filter;
/** Ticks every 15 s — drives progress bars and the guide's now line. */
export const clock = store.clock;

export function setFilter(f: Filter): void {
  store.setFilterSig(f);
  try {
    sessionStorage.setItem(FILTER_KEY, JSON.stringify(f));
  } catch {
    /* ignore */
  }
}

export function filterKey(f: Filter): string {
  return f.kind === "group" ? `g:${f.name}` : f.kind;
}

let loading: Promise<void> | undefined;
let loadedAt = 0;
/** Load (or refresh, if older than 5 min) the channel list + groups. */
export function loadChannels(force = false): Promise<void> {
  if (loading) return loading;
  if (!force && store.channels() && Date.now() - loadedAt < 5 * 60_000) return Promise.resolve();
  store.setError(undefined);
  loading = Promise.all([iptv.channels(), iptv.groups()])
    .then(([c, g]) => {
      loadedAt = Date.now();
      store.setChannels(c);
      store.setGroups(g);
      // A filter pointing at a group that vanished falls back to everything.
      const f = store.filter();
      if (f.kind === "group" && !g.some((x) => x.name === f.name)) setFilter({ kind: "all" });
    })
    .catch((e) => {
      store.setError(e);
    })
    .finally(() => {
      loading = undefined;
    });
  return loading;
}

/** Channels matching the current filter, in list order. */
export function filtered(): IptvChannel[] {
  const all = store.channels() ?? [];
  const f = store.filter();
  if (f.kind === "favorites") return all.filter((c) => c.favorite);
  if (f.kind === "group") return all.filter((c) => c.group === f.name);
  return all;
}

export function channelById(id: number | undefined): IptvChannel | undefined {
  if (id == null) return undefined;
  const l = store.channels();
  return l ? l.find((c) => c.id === id) : undefined;
}

export function channelByNumber(n: number): IptvChannel | undefined {
  const l = store.channels();
  return l ? l.find((c) => c.number === n) : undefined;
}

export async function toggleFavorite(c: IptvChannel): Promise<void> {
  const on = !c.favorite;
  const patch = (v: boolean) => store.setChannels((l) => l && l.map((x) => (x.id === c.id ? { ...x, favorite: v } : x)));
  patch(on);
  try {
    await iptv.favorite(c.id, on);
  } catch (e) {
    patch(!on);
    throw e;
  }
}

// ---- now / next ---------------------------------------------------------------

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

// ---- last channel ---------------------------------------------------------------

export function lastChannel(): number | undefined {
  try {
    const v = Number(localStorage.getItem(LAST_KEY));
    return v > 0 ? v : undefined;
  } catch {
    return undefined;
  }
}
export function rememberChannel(id: number): void {
  try {
    localStorage.setItem(LAST_KEY, String(id));
  } catch {
    /* ignore */
  }
}

// ---- formatting -----------------------------------------------------------------

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

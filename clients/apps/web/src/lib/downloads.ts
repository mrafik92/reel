import { createMemo, createRoot, createSignal } from "solid-js";
import { createDownloadProgressStore, getServerUrl, onServerUrlChange, type DownloadProgress, type ProgressEventSource } from "@reel/ui";
import { ReelEvents, type RequestProgressEvent, type ReelEvent } from "@reel/sdk";
import { api, backend, getToken, invalidate, type DiscoverCard, type DownloadItem, type DownloadsEvent, type ProgressDto, type RequestDto } from "../api";
import { MOCK } from "./mode";
import { onResume } from "./connection";

/**
 * App-wide download progress (SPEC §7.1). One store keyed by tmdb_id, fed by
 * the `/events` WebSocket (or the mock feed) and seeded from
 * `GET /requests/active` on load and on every reconnect.
 */

type Handler = (e: RequestProgressEvent) => void;
const handlers = new Set<Handler>();
const relay = {
  on(type: string, h: Handler) {
    if (type !== "request_progress") return () => undefined;
    handlers.add(h);
    return () => handlers.delete(h);
  },
} as unknown as ProgressEventSource;

export const downloads = createDownloadProgressStore(relay);

/** Library scan progress for admins, keyed by library id. */
const [scans, setScans] = createSignal<Record<number, { scanned: number; total: number; at: number }>>({});
export { scans };

const [connected, setConnected] = createSignal(false);
export { connected as eventsConnected };

// ---------------------------------------------------------------------------
// Radarr/Sonarr queue (`GET /downloads` + `{type:"downloads"}` pushes ~2s while active)

const [queue, setQueue] = createSignal<DownloadItem[]>([]);
export { queue as downloadQueue };
let lastPush = 0;
let lastPoll = 0;
let polling: ReturnType<typeof setInterval> | undefined;

function setQueueItems(items: DownloadItem[]): void {
  const prev = queue();
  setQueue(items);
  // Something finished importing since last time → library views refetch.
  if (prev.some((p) => p.state === "importing" && !items.some((i) => i.id === p.id))) {
    ["home", "libraryItems", "item", "children", "search", "discover"].forEach((k) => invalidate(k));
  }
}

export async function refreshQueue(): Promise<void> {
  lastPoll = Date.now();
  try {
    setQueueItems(await api.downloads());
  } catch {
    /* older server / offline: keep what we have */
  }
}

/** Fallback poll: every 3s while active and the socket is quiet; every 15s when idle. */
function startQueuePolling(): void {
  stopQueuePolling();
  void refreshQueue();
  polling = setInterval(() => {
    if (typeof document !== "undefined" && document.hidden) return;
    const now = Date.now();
    if (now - lastPush < 5000) return;
    const idle = !queue().length;
    if (idle && now - lastPoll < 15000) return;
    void refreshQueue();
  }, 3000);
}
function stopQueuePolling(): void {
  if (polling) clearInterval(polling);
  polling = undefined;
}

export interface Activity {
  state: DownloadItem["state"];
  progress: number;
  eta_seconds: number | null;
  speed_bps: number | null;
  count: number;
}
const RANK: Partial<Record<DownloadItem["state"], number>> = { downloading: 5, importing: 4, queued: 3, stalled: 2, failed: 1 };

/** Size-weighted aggregate (a season pack = one ring). */
export function aggregate(items: DownloadItem[]): Activity | undefined {
  if (!items.length) return undefined;
  let w = 0, p = 0, speed = 0, eta = 0, best = items[0]!.state;
  for (const i of items) {
    const size = i.size_bytes || 1;
    w += size;
    p += i.progress * size;
    speed += i.speed_bps ?? 0;
    eta = Math.max(eta, i.eta_seconds ?? 0);
    if ((RANK[i.state] ?? 0) > (RANK[best] ?? 0)) best = i.state;
  }
  return { state: best, progress: p / w, eta_seconds: eta || null, speed_bps: speed || null, count: items.length };
}

const index = createRoot(() => {
  const group = (key: (i: DownloadItem) => number | null | undefined) =>
    createMemo(() => {
      const m = new Map<number, DownloadItem[]>();
      for (const i of queue()) {
        const k = key(i);
        if (k == null) continue;
        const l = m.get(k);
        if (l) l.push(i);
        else m.set(k, [i]);
      }
      return m;
    });
  return { byTmdb: group((i) => i.tmdb_id), byItem: group((i) => i.library_item_id) };
});

/** Queue activity for a library item (movie/show), for the corner ring on posters. */
export function itemActivity(itemId: number): Activity | undefined {
  const l = index.byItem().get(itemId);
  return l ? aggregate(l) : undefined;
}

/** Ring source for TMDB cards: a live Reel request wins, else raw arr queue activity. */
export function progressFor(tmdbId: number): DownloadProgress | undefined {
  const req = downloads.get(tmdbId);
  if (req && req.state !== "available" && req.state !== "declined") return req;
  const l = index.byTmdb().get(tmdbId);
  const a = l ? aggregate(l) : undefined;
  if (a) return { request_id: String(l![0]!.request_id ?? ""), tmdb_id: tmdbId, state: a.state, progress: a.progress, eta_seconds: a.eta_seconds, speed_bps: a.speed_bps };
  return req;
}

/** Nav indicator: everything in flight. */
export const queueSummary = createRoot(() =>
  createMemo(() => {
    const items = queue().filter((i) => i.state !== "failed");
    const active = items.filter((i) => i.state === "downloading" || i.state === "importing" || i.state === "queued");
    return { count: items.length, failed: queue().length - items.length, active: aggregate(active) };
  }),
);

function dispatch(e: ReelEvent): void {
  if ((e as unknown as DownloadsEvent).type === "downloads") {
    lastPush = Date.now();
    setQueueItems((e as unknown as DownloadsEvent).items);
    return;
  }
  if (e.type === "request_progress") {
    handlers.forEach((h) => h(e));
    // A title just landed: library views should refetch.
    if (e.state === "available") ["home", "search", "libraryItems", "discover"].forEach((k) => invalidate(k));
  } else if (e.type === "scan_progress") {
    const id = Number(e.library_id);
    setScans((s) => ({ ...s, [id]: { scanned: e.scanned, total: e.total, at: Date.now() } }));
    if (e.total && e.scanned >= e.total) invalidate("libraries");
  }
}

export function fromDto(p: ProgressDto): DownloadProgress {
  return {
    request_id: String(p.request_id),
    tmdb_id: p.tmdb_id,
    state: p.state,
    progress: p.progress,
    eta_seconds: p.eta_seconds ?? null,
    speed_bps: p.speed_bps ?? null,
  };
}

/** Pull the REST snapshot (on load and reconnect). */
export async function reseed(): Promise<void> {
  try {
    downloads.seed((await api.activeRequests()).map(fromDto));
  } catch {
    /* offline — the socket will retry */
  }
}

/** Show a ring immediately after Add, before the first event arrives. */
export function noteRequest(r: RequestDto): void {
  downloads.seed([{ request_id: String(r.id), tmdb_id: r.tmdb_id, state: r.status, progress: 0 }]);
}

/** Cards carry their own progress snapshot; use it where the store has nothing newer. */
export function primeFromCards(cards: DiscoverCard[]): void {
  const fresh: DownloadProgress[] = [];
  for (const c of cards) if (c.progress && !downloads.get(c.tmdb_id)) fresh.push(fromDto(c.progress));
  if (fresh.length) downloads.seed(fresh);
}

let stop: (() => void) | undefined;

/** (Re)connect the event stream for the signed-in user. */
export function connectEvents(): void {
  disconnectEvents();
  startQueuePolling();
  if (MOCK) {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend().then((a) => {
      if (cancelled) return;
      const src = (a as unknown as { events?: { on: (t: string, h: (e: ReelEvent) => void) => () => void } }).events;
      off = src?.on("*", dispatch);
      setConnected(true);
      void reseed();
    });
    stop = () => {
      cancelled = true;
      off?.();
      setConnected(false);
    };
    return;
  }
  const token = getToken();
  if (!token) return;
  const ws = new ReelEvents({
    baseUrl: getServerUrl(),
    token,
    onOpen: () => {
      setConnected(true);
      void reseed();
      void refreshQueue();
    },
    onClose: () => setConnected(false),
  });
  const off = ws.onAny(dispatch);
  ws.connect();
  // Back from the background or offline (#81): reconnect now if the socket died, and replace it
  // after a long absence (it may be dead without the browser knowing). The open handler reseeds.
  const offResume = onResume((r) => {
    if (r.awayMs > 60_000 || !ws.alive) ws.kick(true);
    else void refreshQueue();
  });
  stop = () => {
    off();
    offResume();
    ws.close();
    setConnected(false);
  };
}

export function disconnectEvents(): void {
  stopQueuePolling();
  stop?.();
  stop = undefined;
}

// Server address changed (TV first-run / "Change server"): follow it.
onServerUrlChange(() => {
  if (stop) connectEvents();
});

import { createSignal } from "solid-js";
import { createDownloadProgressStore, getServerUrl, onServerUrlChange, type DownloadProgress, type ProgressEventSource } from "@reel/ui";
import { ReelEvents, type RequestProgressEvent, type ReelEvent } from "@reel/sdk";
import { api, backend, getToken, invalidate, type DiscoverCard, type ProgressDto, type RequestDto } from "../api";
import { MOCK } from "./mode";

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

function dispatch(e: ReelEvent): void {
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
    },
    onClose: () => setConnected(false),
  });
  const off = ws.onAny(dispatch);
  ws.connect();
  stop = () => {
    off();
    ws.close();
    setConnected(false);
  };
}

export function disconnectEvents(): void {
  stop?.();
  stop = undefined;
}

// Server address changed (TV first-run / "Change server"): follow it.
onServerUrlChange(() => {
  if (stop) connectEvents();
});

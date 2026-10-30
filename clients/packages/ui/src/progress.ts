import { createStore, reconcile } from "solid-js/store";
import { onCleanup, getOwner } from "solid-js";
import type { ReelEvents, RequestProgressEvent } from "@reel/sdk";

export type DownloadProgress = Omit<RequestProgressEvent, "type">;

/** Anything that can deliver `request_progress` events (a live `ReelEvents` or a mock feed). */
export type ProgressEventSource = Pick<ReelEvents, "on">;

/** Store keyed by tmdb_id, fed from request_progress events. */
export function createDownloadProgressStore(events: ProgressEventSource) {
  const [byTmdb, setByTmdb] = createStore<Record<number, DownloadProgress>>({});
  const off = events.on("request_progress", ({ type: _t, ...p }) => {
    setByTmdb(p.tmdb_id, reconcile(p));
  });
  if (getOwner()) onCleanup(off);
  return {
    byTmdb,
    get: (tmdbId: number): DownloadProgress | undefined => byTmdb[tmdbId],
    /** Seed from REST snapshot (GET /requests/active) on load/reconnect. */
    seed(items: DownloadProgress[]) {
      for (const p of items) setByTmdb(p.tmdb_id, reconcile(p));
    },
    dispose: off,
  };
}

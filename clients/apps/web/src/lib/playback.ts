import { onCleanup } from "solid-js";
import type { PlayerAdapter } from "@reel/ui";
import type { Id } from "@reel/ui";
import { api } from "../api";

/**
 * Report playback position: every 10s while playing, and on pause/end/exit.
 * The transport is stubbed in Phase 1 (see `Api.reportProgress`).
 */
export function useProgressReporter(opts: { itemId: Id; fileId: Id; adapter: PlayerAdapter }): void {
  let pos = 0;
  let dur = 0;
  let paused = true;
  let lastSent = 0;
  const send = (reason: "tick" | "pause" | "ended" | "stop") => {
    if (!dur || pos < 1) return;
    lastSent = Date.now();
    void api.reportProgress({ item_id: opts.itemId, file_id: opts.fileId, position_s: pos, duration_s: dur, paused, reason });
  };
  const offs = [
    opts.adapter.on("timeupdate", (t) => {
      pos = t.positionSec;
      dur = t.durationSec;
      if (!paused && Date.now() - lastSent > 10_000) send("tick");
    }),
    opts.adapter.on("play", () => (paused = false)),
    opts.adapter.on("pause", () => {
      paused = true;
      send("pause");
    }),
    opts.adapter.on("ended", () => {
      paused = true;
      send("ended");
    }),
  ];
  onCleanup(() => {
    send("stop");
    offs.forEach((f) => f());
  });
}

const PREF_KEY = "reel.tracks";
export interface TrackPrefs {
  audio?: number;
  /** -1 = off */
  sub?: number;
}
/** Per-item audio/subtitle choice, remembered locally until the server stores it. */
export function getTrackPrefs(itemId: Id): TrackPrefs {
  try {
    const all = JSON.parse(localStorage.getItem(PREF_KEY) || "{}") as Record<string, TrackPrefs>;
    return all[String(itemId)] ?? {};
  } catch {
    return {};
  }
}
export function setTrackPrefs(itemId: Id, p: TrackPrefs): void {
  try {
    const all = JSON.parse(localStorage.getItem(PREF_KEY) || "{}") as Record<string, TrackPrefs>;
    all[String(itemId)] = { ...all[String(itemId)], ...p };
    localStorage.setItem(PREF_KEY, JSON.stringify(all));
  } catch {
    /* ignore */
  }
}

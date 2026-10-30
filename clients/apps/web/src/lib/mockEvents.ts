import type { RequestProgressEvent, RequestState } from "@reel/sdk";
import type { DownloadProgress, ProgressEventSource } from "@reel/ui";

type Handler = (e: RequestProgressEvent) => void;

export interface MockFeed {
  source: ProgressEventSource;
  /** Restart an item's lifecycle (used by the ring's retry action). */
  retry(tmdbId: number): void;
  stop(): void;
}

interface Sim {
  tmdb_id: number;
  script: "live" | "stall" | "static";
  state: RequestState;
  progress: number;
  t: number;
}

/**
 * Simulated `request_progress` feed (SPEC §7.1) so the ring can be reviewed
 * before Radarr/Sonarr wiring exists. Ticks at 2 Hz.
 */
export function createMockProgressFeed(seed: { tmdb_id: number; state: RequestState; progress?: number; script?: Sim["script"] }[]): MockFeed {
  const handlers = new Set<Handler>();
  const sims: Sim[] = seed.map((s) => ({
    tmdb_id: s.tmdb_id, script: s.script ?? "static", state: s.state, progress: s.progress ?? 0, t: 0,
  }));
  const SIZE = 4.2e9;

  const emit = (s: Sim) => {
    const left = Math.max(0, 1 - s.progress);
    const speed = s.state === "downloading" ? 9.5e6 + Math.sin(s.t / 3) * 3e6 : null;
    const ev: RequestProgressEvent = {
      type: "request_progress",
      request_id: String(s.tmdb_id),
      tmdb_id: s.tmdb_id,
      state: s.state,
      progress: s.progress,
      eta_seconds: s.state === "downloading" && speed ? Math.round((left * SIZE) / speed) : null,
      speed_bps: speed ? Math.round(speed) : null,
    };
    handlers.forEach((h) => h(ev));
  };

  const step = (s: Sim) => {
    s.t++;
    if (s.script === "static") return;
    switch (s.state) {
      case "requested":
      case "approved":
        if (s.t > 3) go(s, "searching");
        break;
      case "searching":
        if (s.t > 5) go(s, "queued");
        break;
      case "queued":
        if (s.t > 3) go(s, "downloading");
        break;
      case "downloading":
        s.progress = Math.min(1, s.progress + 0.018 + Math.random() * 0.02);
        if (s.script === "stall" && s.progress > 0.55 && s.progress < 0.6) {
          s.progress = 0.6;
          go(s, "stalled");
        } else if (s.progress >= 1) go(s, "importing");
        break;
      case "stalled":
        if (s.t > 10) go(s, "downloading");
        break;
      case "importing":
        if (s.t > 5) go(s, "available");
        break;
      case "available":
        if (s.t > 10) {
          s.progress = 0;
          go(s, "requested");
        }
        break;
      case "failed":
      case "not_found":
      case "declined":
        break;
    }
  };
  const go = (s: Sim, st: RequestState) => {
    s.state = st;
    s.t = 0;
  };

  const timer = setInterval(() => {
    for (const s of sims) {
      step(s);
      emit(s);
    }
  }, 500);

  const source = {
    on(type: string, h: Handler) {
      if (type !== "request_progress") return () => undefined;
      handlers.add(h);
      // Initial snapshot (like GET /requests/active).
      setTimeout(() => sims.forEach(emit), 0);
      return () => handlers.delete(h);
    },
  } as unknown as ProgressEventSource;

  return {
    source,
    retry(tmdbId) {
      const s = sims.find((x) => x.tmdb_id === tmdbId);
      if (!s) return;
      s.script = "live";
      s.progress = 0;
      go(s, "searching");
      emit(s);
    },
    stop() {
      clearInterval(timer);
      handlers.clear();
    },
  };
}

export type { DownloadProgress };

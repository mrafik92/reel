/** Development-only, in-memory startup measurements. Values contain no media or account data. */
type Stage = "route" | "decide" | "decided" | "load" | "usable" | "frame";
type Outcome = "rendered" | "failed" | "canceled" | "superseded";

let nextId = 0;
let pendingTap: { at: number; path: string } | undefined;

if (import.meta.env.DEV) {
  document.addEventListener("click", (event) => {
    const anchor = (event.target as Element | null)?.closest("a");
    if (anchor && /\/(?:vod\/)?play\//.test(anchor.pathname)) pendingTap = { at: performance.now(), path: anchor.pathname };
  }, true);
}

export class StartupTiming {
  private readonly id = ++nextId;
  private readonly start = pendingTap?.path === location.pathname && performance.now() - pendingTap.at < 30000
    ? pendingTap.at : performance.now();
  private readonly marks: Partial<Record<Stage, number>> = {};
  private finished = false;
  private mode = "unknown";

  constructor(private readonly source: "library" | "vod") {
    pendingTap = undefined;
    this.mark("route");
  }

  mark(stage: Stage): void {
    if (!this.finished && this.marks[stage] === undefined) this.marks[stage] = Math.round(performance.now() - this.start);
  }

  decided(mode: string): void {
    this.mode = ["direct_play", "remux", "audio_transcode", "transcode", "passthrough"].includes(mode) ? mode : "unknown";
    this.mark("decided");
  }

  finish(outcome: Outcome): void {
    if (this.finished) return;
    this.finished = true;
    console.info("[ReelStartup]", JSON.stringify({ attempt: this.id, outcome, source: this.source, mode: this.mode, ...this.marks }));
  }
}

export function startupTiming(source: "library" | "vod"): StartupTiming | undefined {
  return import.meta.env.DEV ? new StartupTiming(source) : undefined;
}

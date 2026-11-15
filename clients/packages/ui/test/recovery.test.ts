import { describe, expect, test } from "bun:test";
import { RecoveryPolicy, failureKind, hlsConfig } from "../src/recovery";
import { HlsJsAdapter, prefersNativeHls } from "../src/player";

describe("RecoveryPolicy", () => {
  test("network errors reload at the playhead (VOD) with backoff, then rebuild", () => {
    const p = new RecoveryPolicy();
    const a = p.next("network", { live: false, position: 164.2 });
    expect(a.action).toEqual({ kind: "startLoad", position: 164.2 });
    expect(a.delayMs).toBe(1000);
    const b = p.next("network", { live: false, position: 164.2 });
    expect(b.action.kind).toBe("startLoad");
    expect(b.delayMs).toBe(2000);
    const c = p.next("network", { live: false, position: 164.2 });
    expect(c.action).toEqual({ kind: "reload", position: 164.2 });
    expect(c.attempt).toBe(3);
  });

  test("live recovers to the edge, never to a stale playhead", () => {
    const p = new RecoveryPolicy();
    expect(p.next("network", { live: true, position: 90 }).action).toEqual({ kind: "startLoad", position: -1 });
    expect(p.next("other", { live: true, position: 90 }).action).toEqual({ kind: "reload", position: -1 });
  });

  test("media errors: recover, then swap the audio codec, then rebuild", () => {
    let t = 0;
    const p = new RecoveryPolicy({ now: () => t });
    expect(p.next("media", { live: false, position: 10 }).action).toEqual({ kind: "recoverMedia", swapAudio: false });
    t += 2000;
    expect(p.next("media", { live: false, position: 10 }).action).toEqual({ kind: "recoverMedia", swapAudio: true });
    t += 2000;
    expect(p.next("media", { live: false, position: 10 }).action).toEqual({ kind: "reload", position: 10 });
    // Far apart: back to a plain recover.
    t += 60_000;
    expect(p.next("media", { live: false, position: 10 }).action).toEqual({ kind: "recoverMedia", swapAudio: false });
  });

  test("gives up after the budget, and a healthy stretch restores it", () => {
    const p = new RecoveryPolicy({ maxAttempts: 3 });
    for (let i = 0; i < 3; i++) expect(p.next("other", { live: false, position: 1 }).action.kind).toBe("reload");
    expect(p.next("other", { live: false, position: 1 }).action.kind).toBe("giveUp");
    p.healthy();
    expect(p.next("other", { live: false, position: 1 }).action.kind).toBe("reload");
  });

  test("hls.js error types", () => {
    expect(failureKind("networkError")).toBe("network");
    expect(failureKind("mediaError")).toBe("media");
    expect(failureKind("muxError")).toBe("other");
  });

  test("VOD buffers ~90 s ahead; live stays near the edge", () => {
    const vod = hlsConfig({ live: false, startPosition: 30 });
    expect(vod.maxBufferLength).toBe(90);
    expect(vod.startPosition).toBe(30);
    const live = hlsConfig({ live: true, startPosition: -1 });
    expect(live.liveSyncDurationCount).toBe(3);
    expect(live.liveMaxLatencyDurationCount as number).toBeGreaterThan(live.liveSyncDurationCount as number);
  });
});

/** Just enough of an HTMLVideoElement for the direct-play path. */
function fakeVideo() {
  const l = new Map<string, Set<() => void>>();
  const v = {
    src: "",
    currentTime: 0,
    paused: false,
    ended: false,
    readyState: 4,
    duration: 100,
    error: null as { message: string } | null,
    buffered: { length: 0, start: () => 0, end: () => 0 },
    loads: 0,
    canPlayType: () => "",
    addEventListener: (t: string, h: () => void) => {
      if (!l.has(t)) l.set(t, new Set());
      l.get(t)!.add(h);
    },
    removeEventListener: (t: string, h: () => void) => l.get(t)?.delete(h),
    removeAttribute: () => undefined,
    load() {
      v.loads++;
    },
    play: () => Promise.resolve(),
    pause: () => undefined,
    fire: (t: string) => l.get(t)?.forEach((h) => h()),
  };
  return v;
}

describe("HlsJsAdapter recovery (direct play)", () => {
  test("a media error reloads at the playhead instead of stopping", async () => {
    const v = fakeVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    const events: string[] = [];
    a.on("error", (e) => events.push(`error:${e.fatal}`));
    a.on("recovering", (r) => events.push(`recovering:${r ? r.attempt : "done"}`));
    await a.load("http://s/stream/x.mp4", { startPositionSec: 0 });
    expect(v.loads).toBe(1);
    v.currentTime = 164;
    v.fire("timeupdate");
    v.error = { message: "PIPELINE_ERROR_DECODE" };
    v.fire("error");
    await Promise.resolve();
    await Promise.resolve();
    expect(v.loads).toBe(2);
    expect(events).toEqual(["error:false", "recovering:1"]);
    // The resume seek waits for metadata.
    v.currentTime = 0;
    v.fire("loadedmetadata");
    expect(v.currentTime).toBe(164);
    // The picture moves again → recovered.
    v.currentTime = 165;
    v.fire("timeupdate");
    expect(events[events.length - 1]).toBe("recovering:done");
    a.destroy();
  });

  test("isolated errors are recovered, not reported as fatal", async () => {
    const v = fakeVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    let fatal = 0;
    a.on("error", (e) => e.fatal && fatal++);
    await a.load("http://s/stream/x.mp4");
    // Two quick media errors are recovered immediately (no backoff); the third rebuild waits 500 ms.
    v.fire("error");
    await new Promise((r) => setTimeout(r, 1100));
    v.fire("error");
    await new Promise((r) => setTimeout(r, 1100));
    expect(fatal).toBe(0);
    a.destroy();
  });

  test("giving up reports exactly one fatal error", async () => {
    const v = fakeVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement, { maxAttempts: 2 });
    let fatal = 0;
    a.on("error", (e) => e.fatal && fatal++);
    await a.load("http://s/live/x.mp4", { live: true });
    // Two immediate media recoveries, then the third failure gives up (spaced past the 1 s
    // window in which an element error is taken as a duplicate of the one just handled).
    for (let i = 0; i < 3; i++) {
      v.fire("error");
      await new Promise((r) => setTimeout(r, 1050));
    }
    expect(fatal).toBe(1);
    a.destroy();
  }, 10_000);
});

/** A fake whose play()/pause() behave like the element: they flip `paused` and fire the event. */
function playableVideo() {
  const v = fakeVideo();
  const p = Object.assign(v, {
    plays: 0,
    paused: true,
    play: () => {
      p.plays++;
      p.paused = false;
      v.fire("play");
      return Promise.resolve();
    },
    pause: () => {
      p.paused = true;
      v.fire("pause");
    },
  });
  return p;
}
const settle = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

describe("HlsJsAdapter keeps a paused video paused (#100)", () => {
  test("a recovery while paused reloads the stream but doesn't start playback", async () => {
    const v = playableVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    const recovering: (number | null)[] = [];
    a.on("recovering", (r) => recovering.push(r ? r.attempt : null));
    await a.load("http://s/stream/x.mp4", { autoplay: true });
    expect(v.plays).toBe(1);
    v.currentTime = 600;
    v.fire("timeupdate");
    a.pause();
    expect(a.state.wantPlay).toBe(false);
    // e.g. a segment fetched to fill the buffer failed while paused
    v.error = { message: "PIPELINE_ERROR_READ" };
    v.fire("error");
    await settle();
    expect(v.loads).toBe(2); // reloaded…
    expect(v.plays).toBe(1); // …but not played
    expect(v.paused).toBe(true);
    // Data is back: the overlay goes away without waiting for the picture to move.
    v.error = null;
    v.fire("loadedmetadata");
    expect(v.currentTime).toBe(600);
    v.fire("loadeddata");
    expect(recovering).toEqual([1, null]);
    // The viewer presses play later: it plays from where it was.
    await a.play();
    expect(v.plays).toBe(2);
    a.destroy();
  });

  test("a recovery while playing resumes playback", async () => {
    const v = playableVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    await a.load("http://s/stream/x.mp4", { autoplay: true });
    v.currentTime = 30;
    v.fire("timeupdate");
    v.error = { message: "PIPELINE_ERROR_DECODE" };
    v.fire("error");
    await settle();
    expect(v.loads).toBe(2);
    expect(v.plays).toBe(2);
    a.destroy();
  });

  test("a pause from outside the app (media keys) counts; the pause a recovery causes doesn't", async () => {
    const v = playableVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    await a.load("http://s/stream/x.mp4", { autoplay: true });
    v.fire("pause"); // hardware media key: the element pauses itself
    expect(a.state.wantPlay).toBe(false);
    v.fire("play");
    expect(a.state.wantPlay).toBe(true);
    // A failure while playing: the element pauses as it is reset, the viewer still wants it playing.
    v.error = { message: "x" };
    v.fire("error");
    v.fire("pause");
    expect(a.state.wantPlay).toBe(true);
    a.destroy();
  });

  test("a load without autoplay stays paused through recovery", async () => {
    const v = playableVideo();
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    await a.load("http://s/stream/x.mp4", { startPositionSec: 90 });
    v.fire("error");
    await settle();
    expect(v.plays).toBe(0);
    a.destroy();
  });
});

describe("HlsJsAdapter stall diagnostics (#96)", () => {
  test("says whether a segment is stuck in flight or none was requested", () => {
    const v = playableVideo();
    v.currentTime = 105.5;
    const a = new HlsJsAdapter(v as unknown as HTMLVideoElement);
    const priv = a as unknown as { fragLoading?: { sn: number; at: number }; fragLoadedAt: number };
    expect(a.stallDetails(1000)).toBe("stalled at 105.5 s: no data");
    priv.fragLoadedAt = 1000;
    expect(a.stallDetails(31_000)).toBe("stalled at 105.5 s: no segment requested since one arrived 30 s ago");
    priv.fragLoading = { sn: 12, at: 7000 };
    expect(a.stallDetails(31_000)).toBe("stalled at 105.5 s: segment 12 requested 24 s ago and still loading");
    a.destroy();
  });
});

describe("prefersNativeHls", () => {
  const el = (answer: string) => ({ canPlayType: () => answer }) as unknown as HTMLVideoElement;
  const g = globalThis as { MediaSource?: unknown };
  const had = g.MediaSource;

  test("Chrome with built-in HLS still gets hls.js (#91)", () => {
    g.MediaSource = function () {};
    const chrome = { vendor: "Google Inc.", userAgent: "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 Chrome/156.0.0.0 Safari/537.36" };
    expect(prefersNativeHls(el("maybe"), chrome)).toBe(false);
    g.MediaSource = had;
  });

  test("Safari keeps its own player; no MSE means native", () => {
    g.MediaSource = function () {};
    const safari = { vendor: "Apple Computer, Inc.", userAgent: "Mozilla/5.0 (Macintosh) AppleWebKit/605.1.15 Version/19.0 Safari/605.1.15" };
    expect(prefersNativeHls(el("maybe"), safari)).toBe(true);
    expect(prefersNativeHls(el(""), safari)).toBe(false);
    g.MediaSource = undefined;
    expect(prefersNativeHls(el("maybe"), { vendor: "", userAgent: "TV" })).toBe(true);
    g.MediaSource = had;
  });
});

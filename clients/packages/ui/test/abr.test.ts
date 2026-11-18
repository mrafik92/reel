import { describe, expect, test } from "bun:test";
import { abrConfig, fragmentKbps, planLevels, variantIdOf } from "../src/abr";
import { HlsJsAdapter } from "../src/player";

describe("variantIdOf", () => {
  test("reads the variant from a signed level URL", () => {
    expect(variantIdOf("https://reel.test/api/v1/stream/abc/720p/index.m3u8?exp=1&sig=x")).toBe("720p");
    expect(variantIdOf("/api/v1/stream/abc/original/index.m3u8")).toBe("original");
  });
  test("no playlist, no id", () => {
    expect(variantIdOf(undefined)).toBeUndefined();
    expect(variantIdOf("http://s/direct/12")).toBeUndefined();
  });
});

describe("planLevels", () => {
  const ids = ["480p", "720p", "original"];

  test("Auto: start on the server's start variant, no pin, no cap", () => {
    expect(planLevels(ids, { start: "720p", select: {} })).toEqual({ start: 1, pin: -1, cap: -1 });
  });
  test("no start variant: start low, then climb", () => {
    expect(planLevels(ids, {})).toEqual({ start: 0, pin: -1, cap: -1 });
  });
  test("Original pins the original level and starts there", () => {
    expect(planLevels(ids, { start: "480p", select: { pin: "original" } })).toEqual({ start: 2, pin: 2, cap: -1 });
  });
  test("a rung caps ABR at the highest allowed level, and the start never exceeds the cap", () => {
    expect(planLevels(ids, { start: "original", select: { allow: ["480p", "720p"] } })).toEqual({ start: 1, pin: -1, cap: 1 });
  });
  test("allowing every level is no cap", () => {
    expect(planLevels(ids, { select: { allow: ids } })?.cap).toBe(-1);
  });
  test("matches by id, not index: hls.js may have dropped a level", () => {
    expect(planLevels(["480p", "original"], { start: "original", select: { allow: ["480p", "720p"] } })).toEqual({ start: 0, pin: -1, cap: 0 });
  });
  test("a selection the levels can't meet is undefined (the app re-decides)", () => {
    expect(planLevels(["480p", "720p"], { select: { pin: "original" } })).toBeUndefined();
    expect(planLevels(ids, { select: { allow: ["1080p"] } })).toBeUndefined();
    expect(planLevels([], {})).toBeUndefined();
  });
});

describe("fragmentKbps", () => {
  test("timed from the first byte: a variant's ffmpeg starting up isn't a slow link", () => {
    // 1 MB; 2 s waiting for the segment, then 400 ms of transfer → 20 Mbit/s.
    const kbps = fragmentKbps({ loaded: 1_000_000, loading: { start: 0, first: 2000, end: 2400 } });
    expect(kbps).toBe(20_000);
  });
  test("small or instant fragments say nothing", () => {
    expect(fragmentKbps({ loaded: 50_000, loading: { start: 0, first: 10, end: 400 } })).toBeUndefined();
    expect(fragmentKbps({ loaded: 1_000_000, loading: { start: 0, first: 100, end: 101 } })).toBeUndefined();
    expect(fragmentKbps(undefined)).toBeUndefined();
  });
});

describe("abrConfig", () => {
  test("no throwaway bandwidth test (it would start a rung), deferred start, seeded estimate", () => {
    expect(abrConfig({ estimateBps: 12_345_678.4 })).toEqual({
      autoStartLoad: false,
      testBandwidth: false,
      preserveManualLevelOnError: true,
      abrEwmaDefaultEstimate: 12_345_678,
    });
    expect(abrConfig({}).abrEwmaDefaultEstimate).toBeUndefined();
  });
});

// ---- HlsJsAdapter with a fake hls.js ------------------------------------------------------

function fakeVideo() {
  return {
    src: "",
    currentTime: 0,
    paused: true,
    ended: false,
    readyState: 0,
    duration: 0,
    error: null,
    buffered: { length: 0, start: () => 0, end: () => 0 },
    canPlayType: () => "",
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    removeAttribute: () => undefined,
    load: () => undefined,
    play: () => Promise.resolve(),
    pause: () => undefined,
  };
}

const Events = {
  ERROR: "error", LEVEL_LOADED: "levelLoaded", FRAG_BUFFERED: "fragBuffered", FRAG_LOADING: "fragLoading",
  FRAG_LOADED: "fragLoaded", MANIFEST_LOADING: "manifestLoading", MANIFEST_LOADED: "manifestLoaded",
  LEVEL_LOADING: "levelLoading", MANIFEST_PARSED: "manifestParsed", LEVEL_SWITCHED: "levelSwitched",
};

/** Records what the adapter does to hls.js: config, level settings, startLoad. */
class FakeHls {
  static Events = Events;
  static isSupported = () => true;
  static last: FakeHls;
  h = new Map<string, ((e: string, d: any) => void)[]>();
  levels: { uri: string }[] = [];
  autoLevelCapping = -1;
  startLevel = -1;
  manual = -1;
  log: string[] = [];
  started?: number;
  constructor(public config: Record<string, any>) {
    FakeHls.last = this;
  }
  on(e: string, f: (e: string, d: any) => void) {
    this.h.set(e, [...(this.h.get(e) ?? []), f]);
  }
  emit(e: string, d: any = {}) {
    this.h.get(e)?.forEach((f) => f(e, d));
  }
  loadSource() {}
  attachMedia() {}
  destroy() {}
  startLoad(at: number) {
    this.started = at;
  }
  get autoLevelEnabled() {
    return this.manual === -1;
  }
  current = 0;
  get loadLevel() {
    return this.manual >= 0 ? this.manual : this.current;
  }
  set loadLevel(n: number) {
    this.manual = n;
    this.log.push(`loadLevel=${n}`);
  }
  set nextLevel(n: number) {
    this.manual = n;
    this.log.push(`nextLevel=${n}`);
  }
}

async function adaptive(select?: { pin?: string; allow?: string[] }, start = "720p") {
  const a = new HlsJsAdapter(fakeVideo() as unknown as HTMLVideoElement);
  (a as unknown as { HlsCtor: unknown }).HlsCtor = FakeHls;
  const variants: string[] = [];
  a.on("variant", (v) => variants.push(`${v.id}/${v.ids.join(",")}`));
  const kbps: number[] = [];
  a.on("throughput", (t) => kbps.push(t.kbps));
  await a.load("http://s/api/v1/stream/x/master.m3u8?sig=1", { startPositionSec: 42, variants: { start, estimateBps: 8e6, select } });
  const hls = FakeHls.last;
  hls.levels = ["480p", "720p", "original"].map((id) => ({ uri: `http://s/api/v1/stream/x/${id}/index.m3u8?sig=1` }));
  hls.emit(Events.MANIFEST_PARSED);
  return { a, hls, variants, kbps };
}

describe("HlsJsAdapter multi-variant (#111)", () => {
  test("waits for the master, then starts on the start variant at the resume point", async () => {
    const { hls } = await adaptive();
    expect(hls.config.autoStartLoad).toBe(false);
    expect(hls.config.testBandwidth).toBe(false);
    expect(hls.config.abrEwmaDefaultEstimate).toBe(8e6);
    expect(hls.startLevel).toBe(1);
    expect(hls.autoLevelEnabled).toBe(true);
    expect(hls.started).toBe(hls.config.startPosition);
    expect(hls.config.startPosition).toBe(42);
  });

  test("Original is pinned from the start", async () => {
    const { hls } = await adaptive({ pin: "original" });
    expect(hls.startLevel).toBe(2);
    expect(hls.manual).toBe(2);
  });

  test("a rung caps ABR", async () => {
    const { hls } = await adaptive({ allow: ["480p", "720p"] }, "original");
    expect(hls.autoLevelCapping).toBe(1);
    expect(hls.startLevel).toBe(1);
  });

  test("reports the playing variant and fragment throughput", async () => {
    const { hls, variants, kbps } = await adaptive();
    hls.emit(Events.LEVEL_SWITCHED, { level: 2 });
    expect(variants).toEqual(["original/480p,720p,original"]);
    hls.emit(Events.FRAG_LOADED, { frag: { type: "main", sn: 3, stats: { loaded: 1_000_000, loading: { start: 0, first: 100, end: 500 } } } });
    hls.emit(Events.FRAG_LOADED, { frag: { type: "audio", sn: 3, stats: { loaded: 1_000_000, loading: { start: 0, first: 100, end: 500 } } } });
    expect(kbps).toEqual([20_000]);
  });

  test("switches locally: pin, cap below the playing level, back to Auto keeping the buffer", async () => {
    const { a, hls } = await adaptive();
    hls.current = 2; // ABR is on the original
    expect(a.selectVariant({ pin: "original" })).toBe(true);
    expect(hls.log.pop()).toBe("loadLevel=2"); // already there: no flush
    expect(a.selectVariant({ pin: "480p" })).toBe(true);
    expect(hls.log.pop()).toBe("nextLevel=0");
    expect(a.selectVariant({})).toBe(true);
    expect(hls.log.pop()).toBe("loadLevel=-1");
    expect(hls.autoLevelCapping).toBe(-1);
    hls.current = 2;
    expect(a.selectVariant({ allow: ["480p", "720p"] })).toBe(true);
    expect(hls.autoLevelCapping).toBe(1);
    expect(hls.log.pop()).toBe("nextLevel=-1"); // drop what's buffered above the cap
  });

  test("a selection the master can't meet is refused", async () => {
    const { a } = await adaptive();
    expect(a.selectVariant({ pin: "1080p" })).toBe(false);
  });

  test("single-variant sessions don't switch locally", async () => {
    const a = new HlsJsAdapter(fakeVideo() as unknown as HTMLVideoElement);
    (a as unknown as { HlsCtor: unknown }).HlsCtor = FakeHls;
    await a.load("http://s/api/v1/stream/x/master.m3u8");
    expect(FakeHls.last.config.autoStartLoad).toBeUndefined();
    expect(a.selectVariant({})).toBe(false);
  });
});

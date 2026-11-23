import { describe, expect, mock, test } from "bun:test";
import { uiMock } from "./uiMock";

mock.module("@reel/ui", uiMock);
const store = new Map<string, string>();
(globalThis as { localStorage?: unknown }).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
};
const abr = await import("../src/lib/abr");
const { StallCounter, bandwidthEstimate, capEstimate, estimateKbps, estimateKey, isAdaptive, nextEstimate, recordThroughput, variantClass, variantSelect } = abr;
import type { DecideResponse, VariantOption } from "../src/api";

const v = (id: VariantOption["id"], width: number, height: number, transcode = id !== "original"): VariantOption => ({
  id, label: id, width, height, bandwidth_kbps: 0, average_kbps: 0, transcode,
});
/** Meet Joe Black (HEVC 1080p, 1920×1038): original + 720p + 480p rungs. */
const MJB = [v("480p", 854, 462), v("720p", 1280, 692), v("original", 1920, 1038)];
/** A 4K tone-map: transcode-only, 1080p top rung. */
const TONEMAP = [v("480p", 854, 480), v("720p", 1280, 720), v("1080p", 1920, 1080)];

describe("bandwidth estimate", () => {
  test("drops count more than recoveries", () => {
    expect(nextEstimate(undefined, 5000)).toBe(5000);
    expect(nextEstimate(10_000, 5000)).toBe(10_000 * 0.65 + 5000 * 0.35);
    expect(nextEstimate(10_000, 20_000)).toBe(10_000 * 0.8 + 20_000 * 0.2);
    expect(nextEstimate(10_000, 20_000, 10)).toBe(10_000 * 0.4 + 20_000 * 0.6); // weight capped at 0.6
  });

  test("per server, stored, and sent rounded; bad samples are ignored", () => {
    expect(estimateKey()).toBe("reel.bw.https://reel.test");
    recordThroughput(NaN);
    recordThroughput(-3);
    expect(bandwidthEstimate()).toBeUndefined();
    recordThroughput(12_345.6, 1, 1_000_000);
    expect(estimateKbps()).toBe(12_346);
    expect(JSON.parse(store.get("reel.bw.https://reel.test")!)).toEqual({ kbps: 12_346, at: 1_000_000 });
  });

  test("a stalled direct play holds the estimate below the file (Q1 needs ≥ 1.5×)", () => {
    recordThroughput(40_000, 10, 2_000_000);
    capEstimate(6_500, 2_000_001);
    expect(bandwidthEstimate()).toBe(6_500);
    capEstimate(9_000, 2_000_002); // never raises it
    expect(bandwidthEstimate()).toBe(6_500);
  });
});

describe("quality → variants (#87 on #111)", () => {
  test("size class: a rung is its own id, an original the box it fits", () => {
    expect(variantClass(v("original", 1920, 1038))).toBe("1080p");
    expect(variantClass(v("original", 1920, 804))).toBe("1080p");
    expect(variantClass(v("original", 1280, 720))).toBe("720p");
    expect(variantClass(v("original", 3840, 2160))).toBeUndefined();
    expect(variantClass(v("720p", 1280, 536))).toBe("720p");
  });

  test("Auto is ABR over everything", () => {
    expect(variantSelect("auto", MJB)).toEqual({});
  });

  test("Original pins the original; a transcode-only master can't (the app re-decides)", () => {
    expect(variantSelect("original", MJB)).toEqual({ pin: "original" });
    expect(variantSelect("original", TONEMAP)).toBeNull();
  });

  test("a rung caps ABR at that size, never upscaling", () => {
    expect(variantSelect("720p", MJB)).toEqual({ allow: ["480p", "720p"] });
    expect(variantSelect("480p", MJB)).toEqual({ allow: ["480p"] });
    // The 1080p original is "1080p": everything is allowed.
    expect(variantSelect("1080p", MJB)).toEqual({ allow: ["480p", "720p", "original"] });
    expect(variantSelect("1080p", TONEMAP)).toEqual({ allow: ["480p", "720p", "1080p"] });
  });

  test("a rung the master doesn't list re-decides (single-variant at that rung)", () => {
    expect(variantSelect("720p", [v("480p", 854, 480), v("original", 1920, 1080)])).toBeNull();
    expect(variantSelect("1080p", [v("480p", 854, 480), v("original", 3840, 2160)])).toBeNull();
  });

  test("only a session with several variants and a start variant is adaptive", () => {
    const d = (o: Partial<DecideResponse>) => o as DecideResponse;
    expect(isAdaptive(d({ variants: MJB, start_variant: "480p" }))).toBe(true);
    expect(isAdaptive(d({ variants: [MJB[2]!] }))).toBe(false); // REEL_ABR=0, casts, IPTV VOD
    expect(isAdaptive(d({ variants: MJB }))).toBe(false);
    expect(isAdaptive(undefined)).toBe(false);
  });
});

describe("StallCounter (direct play → adaptive, #1)", () => {
  test("two stalls in 30 s switch, once", () => {
    const s = new StallCounter();
    expect(s.stall(10_000)).toBe(false);
    expect(s.stall(25_000)).toBe(true);
    expect(s.stall(26_000)).toBe(false); // counting starts over
  });
  test("stalls further apart don't add up", () => {
    const s = new StallCounter();
    expect(s.stall(10_000)).toBe(false);
    expect(s.stall(45_000)).toBe(false);
    expect(s.stall(60_000)).toBe(true);
  });
  test("waiting right after a start or a seek isn't a stall", () => {
    const s = new StallCounter();
    s.settle(10_000);
    expect(s.stall(11_000)).toBe(false);
    expect(s.stall(12_000)).toBe(false);
    expect(s.stall(14_000)).toBe(false);
    expect(s.stall(15_000)).toBe(true);
  });
});

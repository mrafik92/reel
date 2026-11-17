import { describe, expect, test } from "bun:test";
import { SEEK_STREAK, SINGLE_CLICK_DELAY, createDoubleTap, pointerKind, zoneOf } from "../src/lib/tap";

describe("pointerKind", () => {
  test("only a mouse pauses; touch, pen and unknown take the controls path", () => {
    expect(pointerKind("mouse")).toBe("mouse");
    expect(pointerKind("touch")).toBe("touch");
    expect(pointerKind("pen")).toBe("touch");
    expect(pointerKind("")).toBe("touch");
    expect(pointerKind(undefined)).toBe("touch");
  });
});

describe("zoneOf", () => {
  test("splits the picture into thirds", () => {
    expect(zoneOf(10, 390)).toBe(-1);
    expect(zoneOf(195, 390)).toBe(0);
    expect(zoneOf(380, 390)).toBe(1);
  });
});

describe("createDoubleTap", () => {
  test("a lone tap is a single", () => {
    const d = createDoubleTap();
    expect(d.tap(0, 1)).toEqual({ kind: "single" });
    expect(d.tap(SINGLE_CLICK_DELAY + 50, 1)).toEqual({ kind: "single" });
  });

  test("a double-tap on a side seeks ±10 s", () => {
    const d = createDoubleTap();
    d.tap(0, -1);
    expect(d.tap(150, -1)).toEqual({ kind: "seek", delta: -10, total: -10, zone: -1 });
    const e = createDoubleTap();
    e.tap(0, 1);
    expect(e.tap(150, 1)).toEqual({ kind: "seek", delta: 10, total: 10, zone: 1 });
  });

  test("a double-tap in the centre is a double", () => {
    const d = createDoubleTap();
    d.tap(0, 0);
    expect(d.tap(150, 0)).toEqual({ kind: "double" });
    expect(d.tap(300 + SINGLE_CLICK_DELAY, 0)).toEqual({ kind: "single" });
  });

  test("further taps during the streak add up", () => {
    const d = createDoubleTap();
    d.tap(0, 1);
    d.tap(150, 1);
    expect(d.tap(600, 1)).toEqual({ kind: "seek", delta: 10, total: 20, zone: 1 });
    expect(d.tap(1200, 1)).toEqual({ kind: "seek", delta: 10, total: 30, zone: 1 });
    // Streak over: back to single taps.
    expect(d.tap(1200 + SEEK_STREAK + 1, 1)).toEqual({ kind: "single" });
  });

  test("the other side starts a new streak; the centre ends it", () => {
    const d = createDoubleTap();
    d.tap(0, 1);
    d.tap(150, 1);
    expect(d.tap(400, -1)).toEqual({ kind: "seek", delta: -10, total: -10, zone: -1 });
    expect(d.tap(600, 0)).toEqual({ kind: "single" });
    // The centre tap was a fresh first tap: a side tap right after it pairs with it.
    expect(d.tap(700, 1)).toEqual({ kind: "seek", delta: 10, total: 10, zone: 1 });
  });
});

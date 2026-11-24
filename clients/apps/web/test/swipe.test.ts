import { describe, expect, test } from "bun:test";
import { SWIPE_MAX_MS, SWIPE_MIN, SWIPE_RATIO, SWIPE_SLOP, classifySwipe, createSwipe, type SwipePoint } from "../src/lib/swipe";

const pt = (x: number, y: number, identifier = 0): SwipePoint => ({ identifier, clientX: x, clientY: y });

describe("classifySwipe", () => {
  test("swipe up is the next channel, swipe down the previous one (like Channel Up/Down)", () => {
    expect(classifySwipe(0, -120, 200)).toBe(1);
    expect(classifySwipe(0, 120, 200)).toBe(-1);
  });

  test("needs SWIPE_MIN of vertical travel", () => {
    expect(classifySwipe(0, -(SWIPE_MIN - 1), 200)).toBe(0);
    expect(classifySwipe(0, -SWIPE_MIN, 200)).toBe(1);
    expect(classifySwipe(0, SWIPE_MIN - 1, 200)).toBe(0);
  });

  test("taps and small drags are not swipes", () => {
    expect(classifySwipe(0, 0, 80)).toBe(0);
    expect(classifySwipe(4, -6, 120)).toBe(0);
    expect(classifySwipe(-20, 30, 300)).toBe(0);
  });

  test("must be clearly vertical", () => {
    // Exactly at the ratio still counts; a little more sideways doesn't.
    expect(classifySwipe(50, -50 * SWIPE_RATIO, 200)).toBe(1);
    expect(classifySwipe(51, -50 * SWIPE_RATIO, 200)).toBe(0);
    // Horizontal drags (seek, volume) are ignored however long they are.
    expect(classifySwipe(300, 10, 200)).toBe(0);
    expect(classifySwipe(-300, -100, 200)).toBe(0);
  });

  test("a slow drag is not a swipe", () => {
    expect(classifySwipe(0, -200, SWIPE_MAX_MS)).toBe(1);
    expect(classifySwipe(0, -200, SWIPE_MAX_MS + 1)).toBe(0);
  });
});

describe("createSwipe", () => {
  test("one finger up then lifted: next channel", () => {
    const s = createSwipe();
    s.start([pt(100, 400)], 0);
    s.move([pt(102, 330)]);
    s.move([pt(103, 280)]);
    expect(s.end([pt(103, 280)], 0, 180)).toBe(1);
    expect(s.active()).toBe(false);
  });

  test("one finger down: previous channel", () => {
    const s = createSwipe();
    s.start([pt(100, 200)], 0);
    expect(s.end([pt(96, 330)], 0, 200)).toBe(-1);
  });

  test("a tap stays a tap", () => {
    const s = createSwipe();
    s.start([pt(100, 200)], 0);
    expect(s.end([pt(101, 202)], 0, 90)).toBe(0);
  });

  test("a second finger cancels the gesture", () => {
    const s = createSwipe();
    s.start([pt(100, 400, 1)], 0);
    s.start([pt(100, 400, 1), pt(200, 400, 2)], 20);
    expect(s.active()).toBe(false);
    expect(s.end([pt(200, 250, 2)], 1, 150)).toBe(0);
    expect(s.end([pt(100, 250, 1)], 0, 160)).toBe(0);
  });

  test("two fingers in a move cancel it too", () => {
    const s = createSwipe();
    s.start([pt(100, 400, 1)], 0);
    s.move([pt(100, 380, 1), pt(200, 380, 2)]);
    expect(s.end([pt(100, 250, 1)], 0, 150)).toBe(0);
  });

  test("lifting one of several fingers is not a swipe", () => {
    const s = createSwipe();
    s.start([pt(100, 400, 1)], 0);
    expect(s.end([pt(100, 250, 1)], 1, 150)).toBe(0);
  });

  test("a drag that went sideways first stays cancelled even if it turns vertical", () => {
    const s = createSwipe();
    s.start([pt(100, 400)], 0);
    s.move([pt(100 + SWIPE_SLOP + 5, 402)]);
    expect(s.active()).toBe(false);
    s.move([pt(120, 200)]);
    expect(s.end([pt(120, 200)], 0, 250)).toBe(0);
  });

  test("small sideways wobble inside the slop is tolerated", () => {
    const s = createSwipe();
    s.start([pt(100, 400)], 0);
    s.move([pt(100 + SWIPE_SLOP, 398)]);
    expect(s.active()).toBe(true);
    expect(s.end([pt(110, 250)], 0, 220)).toBe(1);
  });

  test("cancel (touchcancel, or the gesture became disabled) drops it", () => {
    const s = createSwipe();
    s.start([pt(100, 400)], 0);
    s.cancel();
    expect(s.end([pt(100, 200)], 0, 150)).toBe(0);
  });

  test("an end for an unknown finger is ignored and keeps tracking", () => {
    const s = createSwipe();
    s.start([pt(100, 400, 7)], 0);
    expect(s.end([pt(100, 200, 9)], 0, 150)).toBe(0);
    expect(s.end([pt(100, 200, 7)], 0, 160)).toBe(1);
  });

  test("each new gesture starts fresh", () => {
    const s = createSwipe();
    s.start([pt(100, 400)], 0);
    expect(s.end([pt(100, 300)], 0, 100)).toBe(1);
    s.start([pt(100, 300)], 500);
    expect(s.end([pt(100, 420)], 0, 650)).toBe(-1);
  });
});

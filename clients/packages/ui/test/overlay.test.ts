import { describe, expect, test } from "bun:test";
import { overlayTarget } from "../src/overlay";

describe("overlayTarget (#94)", () => {
  const body = { id: "body" } as unknown as HTMLElement;
  const stage = { id: "stage" } as unknown as Element;

  test("mounts into <body> when nothing is full screen", () => {
    expect(overlayTarget({ body, fullscreenElement: null })).toBe(body);
  });

  test("mounts inside the full-screen element so sheets and toasts are painted", () => {
    expect(overlayTarget({ body, fullscreenElement: stage })).toBe(stage);
  });

  test("falls back to Safari's prefixed full-screen element", () => {
    expect(overlayTarget({ body, fullscreenElement: undefined, webkitFullscreenElement: stage })).toBe(stage);
  });
});

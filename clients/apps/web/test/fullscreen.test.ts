import { describe, expect, test } from "bun:test";
import { leaveFullscreen, playerFullscreen } from "../src/lib/fullscreen";

/** A tiny stand-in for `document` and elements: tracks what is full screen. */
function world() {
  const doc = {
    fullscreenElement: null as Element | null,
    exits: 0,
    exitFullscreen(): Promise<void> {
      doc.exits++;
      doc.fullscreenElement = null;
      return Promise.resolve();
    },
  };
  const el = (name: string) => {
    const e = {
      name,
      requestFullscreen(): Promise<void> {
        doc.fullscreenElement = e as unknown as Element;
        return Promise.resolve();
      },
    };
    return e;
  };
  return { doc, el };
}

describe("leaveFullscreen", () => {
  test("exits only when something is full screen", () => {
    const { doc, el } = world();
    leaveFullscreen(doc);
    expect(doc.exits).toBe(0);
    void el("x").requestFullscreen();
    leaveFullscreen(doc);
    expect(doc.exits).toBe(1);
    expect(doc.fullscreenElement).toBeNull();
  });
});

describe("playerFullscreen", () => {
  test("toggles the route's host, not the view's stage", () => {
    const { doc, el } = world();
    const host = el("host");
    const stage = el("stage");
    const fs = playerFullscreen({ doc, host: () => host, own: () => stage });
    fs.toggle();
    expect(doc.fullscreenElement).toBe(host as unknown as Element);
    fs.toggle();
    expect(doc.fullscreenElement).toBeNull();
  });

  test("next episode: the old view's cleanup keeps full screen, the route's cleanup leaves it", () => {
    const { doc, el } = world();
    const host = el("host");
    const first = playerFullscreen({ doc, host: () => host, own: () => el("stage 1") });
    first.toggle();
    // Next → same route, new item: the keyed Show disposes the old view, mounts a new one.
    first.release();
    const second = playerFullscreen({ doc, host: () => host, own: () => el("stage 2") });
    expect(doc.fullscreenElement).toBe(host as unknown as Element);
    expect(doc.exits).toBe(0);
    // The new view's button still controls the same full screen.
    expect(second.target()).toBe(host);
    // Back / ✕: the view goes, then the route wrapper leaves full screen.
    second.release();
    leaveFullscreen(doc);
    expect(doc.fullscreenElement).toBeNull();
    expect(doc.exits).toBe(1);
  });

  test("without a host the view falls back to its stage and leaves full screen itself", () => {
    const { doc, el } = world();
    const stage = el("stage");
    const fs = playerFullscreen({ doc, host: undefined, own: () => stage });
    fs.toggle();
    expect(doc.fullscreenElement).toBe(stage as unknown as Element);
    fs.release();
    expect(doc.fullscreenElement).toBeNull();
  });

  test("a host that isn't mounted yet falls back to the stage", () => {
    const { doc, el } = world();
    const stage = el("stage");
    const fs = playerFullscreen({ doc, host: () => undefined, own: () => stage });
    expect(fs.target()).toBe(stage);
  });
});

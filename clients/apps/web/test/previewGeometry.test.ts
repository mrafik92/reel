import { describe, expect, test } from "bun:test";
import {
  previewPlacement, previewWidth, qualityBadge, translateXOf,
  type PlacementInput, type PreviewKind, type Rect,
} from "../src/lib/previewGeometry";

// A 1440 × 900 desktop: 16 px rem, 3 rem gutters, 11 rem posters, 18 rem landscape tiles.
const REM = 16;
const VW = 1440;
const VH = 900;
const G = 48;
const POSTER = { w: 176, h: 264 };
const LAND = { w: 288, h: 162 };

function place(kind: PreviewKind, tile: Rect, over: Partial<PlacementInput> = {}) {
  const width = previewWidth(kind, tile.width, REM, VW, G);
  return previewPlacement({
    kind, tile, width, height: 330, headBottom: tile.top - 20, viewportWidth: VW, viewportHeight: VH,
    gutter: G, minTop: 72, gap: 4, ...over,
  });
}
const poster = (left: number, top = 400): Rect => ({ left, top, width: POSTER.w, height: POSTER.h });

describe("previewWidth", () => {
  test("posters: 1.9 × the tile, at least 20 rem", () => {
    expect(previewWidth("poster", 176, REM, VW, G)).toBeCloseTo(334.4);
    expect(previewWidth("poster", 120, REM, VW, G)).toBe(320);
    expect(previewWidth("ranked", 176, REM, VW, G)).toBeCloseTo(334.4);
  });
  test("landscape tiles: 1.32 × the tile, so the card covers no more than its neighbours' gaps", () => {
    expect(previewWidth("landscape", LAND.w, REM, VW, G)).toBeCloseTo(380.16);
  });
  test("never wider than the space between the gutters", () => {
    expect(previewWidth("landscape", 1400, REM, VW, G)).toBe(VW - 2 * G);
  });
});

describe("previewPlacement", () => {
  test("a middle tile: centred on the tile, neighbours part evenly", () => {
    const p = place("poster", poster(600));
    expect(p.left).toBeCloseTo(600 + 88 - 167.2);
    expect(p.before).toBeCloseTo(-79.2);
    expect(p.after).toBeCloseTo(79.2);
    // The card grows from the tile's centre.
    expect(p.left + p.originX).toBeCloseTo(688);
    expect(p.top + p.originY).toBeCloseTo(400 + 132);
  });

  test("the first tile: clamped at the left gutter, only the tiles after it move", () => {
    const p = place("poster", poster(G));
    expect(p.left).toBe(G);
    expect(p.before).toBe(0);
    expect(p.after).toBeCloseTo(334.4 - 176);
  });

  test("the last visible tile: clamped at the right gutter, only the tiles before it move", () => {
    const p = place("poster", poster(VW - G - POSTER.w));
    expect(p.left + 334.4).toBeCloseTo(VW - G);
    expect(p.before).toBeCloseTo(-(334.4 - 176));
    expect(p.after).toBeCloseTo(0);
  });

  test("a tile half under the left gutter (row scrolled): the tiles before it move right, to the card's edge", () => {
    const p = place("poster", poster(-40));
    expect(p.left).toBe(G);
    expect(p.before).toBe(88);
    expect(p.after).toBeCloseTo(G + 334.4 - (-40 + 176));
  });

  test("neighbours end exactly at the card's edges: nothing is covered", () => {
    const tile = poster(700);
    const p = place("poster", tile);
    const gap = 8;
    const prevRight = tile.left - gap + p.before;
    const nextLeft = tile.left + tile.width + gap + p.after;
    expect(prevRight).toBeCloseTo(p.left - gap);
    expect(nextLeft).toBeCloseTo(p.left + 334.4 + gap);
  });

  test("never covers its own row heading: it grows downward instead", () => {
    const p = place("poster", poster(600, 400), { headBottom: 380 });
    // Centred it would start at 532 - 165 = 367, over the heading.
    expect(p.top).toBe(384);
  });

  test("a row low on the screen: the viewport wins, the whole card shows", () => {
    const p = place("poster", poster(600, 700), { headBottom: 680 });
    expect(p.top).toBe(VH - 330 - 4);
  });

  test("never above the top bar", () => {
    const p = place("poster", poster(600, 40), { headBottom: null });
    expect(p.top).toBe(72);
  });

  test("numbered row: opens at the poster's left edge, beside its numeral", () => {
    const p = place("ranked", poster(600));
    expect(p.left).toBe(600);
    expect(p.before).toBe(0);
    expect(p.after).toBeCloseTo(334.4 - 176);
  });

  test("numbered row at the right gutter: the numeral moves left with the earlier tiles", () => {
    const p = place("ranked", poster(1200));
    // 1200 + 176 ends 16 px inside the gutter: the card is clamped, the next tile moves 16 px.
    expect(p.left + 334.4).toBeCloseTo(VW - G);
    expect(p.before).toBeCloseTo(VW - G - 334.4 - 1200);
    expect(p.before).toBeLessThan(0);
    expect(p.after).toBeCloseTo(16);
  });

  test("landscape: the card covers about a third of each neighbour's slot, which then moves aside", () => {
    const tile = { left: 400, top: 300, width: LAND.w, height: LAND.h };
    const p = place("landscape", tile);
    expect(p.before).toBeCloseTo(-(380.16 - 288) / 2);
    expect(p.after).toBeCloseTo((380.16 - 288) / 2);
  });
});

describe("translateXOf", () => {
  test("reads the x offset of a computed transform", () => {
    expect(translateXOf("none")).toBe(0);
    expect(translateXOf("")).toBe(0);
    expect(translateXOf(undefined)).toBe(0);
    expect(translateXOf("matrix(1, 0, 0, 1, -79.2, 0)")).toBeCloseTo(-79.2);
    expect(translateXOf("matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 42.5, 3, 0, 1)")).toBeCloseTo(42.5);
  });
});

describe("qualityBadge", () => {
  const f = (width: number, hdr = "none") => ({ video: { width, hdr } });
  test("4K from about 3800 px wide, HDR when not SDR, nothing for ordinary HD", () => {
    expect(qualityBadge([f(3840, "hdr10")])).toBe("4K · HDR");
    expect(qualityBadge([f(3840)])).toBe("4K");
    expect(qualityBadge([f(1920, "dv")])).toBe("HDR");
    expect(qualityBadge([f(1920)])).toBe("");
    expect(qualityBadge([f(3600)])).toBe("");
  });
  test("no files or no video stream: nothing", () => {
    expect(qualityBadge([])).toBe("");
    expect(qualityBadge(undefined)).toBe("");
    expect(qualityBadge([{ video: null }])).toBe("");
  });
  test("any file counts: a 4K version next to an HD one", () => {
    expect(qualityBadge([f(1920), f(3840, "hlg")])).toBe("4K · HDR");
  });
});

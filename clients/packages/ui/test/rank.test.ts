import { describe, expect, test } from "bun:test";
import { RANK_CAP, RANK_H, rankNumeral } from "../src/components/rank";

describe("rankNumeral", () => {
  test("a digit's cap height is the poster's height", () => {
    const m = rankNumeral(3);
    expect(m.fontSize * RANK_CAP).toBeCloseTo(RANK_H, 0);
    expect(m.y).toBeLessThan(RANK_H);
    expect(m.y).toBeGreaterThan(RANK_H - 15);
  });

  test("natural proportions: each numeral is as wide as its own glyph, no stretching", () => {
    // 1 is the narrowest digit, 0 and 6 among the widest.
    expect(rankNumeral(1).width).toBeLessThan(rankNumeral(7).width);
    expect(rankNumeral(7).width).toBeLessThan(rankNumeral(6).width);
    // About 0.44 em plus the outline for one digit.
    expect(rankNumeral(6).width).toBeCloseTo(0.445 * (RANK_H / RANK_CAP) + 42, -1);
  });

  test("10 is tightened", () => {
    const ten = rankNumeral(10);
    expect(ten.letterSpacing).toBeLessThan(0);
    expect(rankNumeral(9).letterSpacing).toBe(0);
    const loose = rankNumeral(1).width + rankNumeral(0).width - 42;
    expect(ten.width).toBeLessThan(loose);
  });
});

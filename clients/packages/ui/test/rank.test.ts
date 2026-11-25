import { describe, expect, test } from "bun:test";
import { RANK_H, RANK_INK_BOTTOM, RANK_INK_TOP, RANK_STROKE, rankNumeral } from "../src/components/rank";

describe("rankNumeral", () => {
  test("the tallest digit and its outline fill the poster's height, no more", () => {
    const m = rankNumeral(3);
    const top = m.y - RANK_INK_TOP * m.fontSize - RANK_STROKE / 2;
    const bottom = m.y + RANK_INK_BOTTOM * m.fontSize + RANK_STROKE / 2;
    expect(top).toBeCloseTo(0, 0);
    expect(bottom).toBeCloseTo(RANK_H, 0);
  });

  test("natural proportions: each numeral is as wide as its own glyph, no stretching", () => {
    // 1 is the narrowest digit, 0 and 6 among the widest.
    expect(rankNumeral(1).width).toBeLessThan(rankNumeral(7).width);
    expect(rankNumeral(7).width).toBeLessThan(rankNumeral(6).width);
    // About 0.44 em plus the outline for one digit.
    expect(rankNumeral(6).width).toBeCloseTo(0.445 * rankNumeral(6).fontSize + RANK_STROKE, -1);
  });

  test("10 is tightened", () => {
    const ten = rankNumeral(10);
    expect(ten.letterSpacing).toBeLessThan(0);
    expect(rankNumeral(9).letterSpacing).toBe(0);
    const loose = rankNumeral(1).width + rankNumeral(0).width - RANK_STROKE;
    expect(ten.width).toBeLessThan(loose);
  });
});

import { describe, expect, test } from "bun:test";
import { playerHeading, volumeIcon } from "../src/lib/playerChrome";

describe("volumeIcon", () => {
  test("follows the level: off, quiet, loud", () => {
    expect(volumeIcon(0)).toBe("mute");
    expect(volumeIcon(-0.1)).toBe("mute");
    expect(volumeIcon(0.05)).toBe("volume");
    expect(volumeIcon(0.45)).toBe("volume");
    expect(volumeIcon(0.5)).toBe("volumeHigh");
    expect(volumeIcon(1)).toBe("volumeHigh");
  });
});

describe("playerHeading", () => {
  const film = { kind: "movie", title: "The Lantern Coast", year: 2025, code: "", runtime: "2h 8m" };
  const episode = { kind: "episode", title: "The Secret Atlas", year: 2024, code: "S3 · E3", runtime: "48m" };

  test("a film shows its year and runtime under the title", () => {
    expect(playerHeading(film)).toEqual({ title: "The Lantern Coast", line: "2025 · 2h 8m" });
    expect(playerHeading({ ...film, year: null })).toEqual({ title: "The Lantern Coast", line: "2h 8m" });
    expect(playerHeading({ ...film, year: null, runtime: "" })).toEqual({ title: "The Lantern Coast", line: "" });
  });

  test("an episode shows the series first, then its number and name", () => {
    expect(playerHeading({ ...episode, series: "Harbour Lights" })).toEqual({ title: "Harbour Lights", line: "S3 E3 · The Secret Atlas" });
  });

  test("until the series name is known the episode keeps its own title", () => {
    expect(playerHeading(episode)).toEqual({ title: "The Secret Atlas", line: "S3 E3" });
  });

  test("an episode without numbers has no stray separator", () => {
    expect(playerHeading({ ...episode, code: "", series: "Harbour Lights" })).toEqual({ title: "Harbour Lights", line: "The Secret Atlas" });
  });

  test("a line from the page that opened the player wins", () => {
    expect(playerHeading({ ...episode, series: "Harbour Lights", eyebrow: "IPTV · S1 · E2" })).toEqual({
      title: "The Secret Atlas",
      line: "IPTV · S1 · E2",
    });
  });
});

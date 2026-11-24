/** Pure pieces of the player's chrome (#293), kept apart from the components so they can be tested. */

/** Speaker glyph for a level: off, quiet (one wave) or loud (two). */
export function volumeIcon(level: number): "mute" | "volume" | "volumeHigh" {
  return level <= 0 ? "mute" : level < 0.5 ? "volume" : "volumeHigh";
}

export interface HeadingInput {
  kind: string;
  title: string;
  year?: number | null;
  /** `episodeCode()` of the item: "S2 · E5", or "" when it has no numbers. */
  code: string;
  /** `formatRuntime()` of the item: "2h 1m", or "" when unknown. */
  runtime: string;
  /** Series name, once it is known. */
  series?: string;
  /** A line supplied by the page that opened the player (IPTV on demand). */
  eyebrow?: string;
}

/**
 * The top bar: a name and one quiet line under it. An episode shows its series first, then
 * "S2 E5 · Episode title"; a film shows "2026 · 2h 1m". A supplied eyebrow always is the line.
 */
export function playerHeading(i: HeadingInput): { title: string; line: string } {
  if (i.eyebrow) return { title: i.title, line: i.eyebrow };
  if (i.kind === "episode") {
    const code = i.code.replace(" · ", " ");
    if (i.series) return { title: i.series, line: [code, i.title].filter(Boolean).join(" · ") };
    return { title: i.title, line: code };
  }
  return { title: i.title, line: [i.year ? String(i.year) : "", i.runtime].filter(Boolean).join(" · ") };
}

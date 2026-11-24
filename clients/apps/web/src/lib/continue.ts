import { formatRemaining, type ItemSummary } from "@reel/ui";

/** `S1:E4`, the compact episode code of Continue watching and the title page. */
export function episodeTag(it: Pick<ItemSummary, "season_no" | "episode_no">): string {
  if (it.season_no == null && it.episode_no == null) return "";
  return `S${it.season_no ?? 0}:E${it.episode_no ?? 0}`;
}

/**
 * A Continue watching entry as its tile shows it: an episode wears its series' title and art, while
 * `id` stays the episode, so the link and Play still resume it.
 */
export function continueTile(it: ItemSummary): ItemSummary {
  const s = it.series;
  if (!s) return it;
  return {
    ...it,
    title: s.title,
    poster_url: s.poster_url ?? it.poster_url,
    backdrop_url: s.backdrop_url ?? it.backdrop_url,
  };
}

/** `S1:E4 · 34 min left`, `1h 39m left`, or the percentage when the runtime is unknown. */
export function continueSubtitle(it: ItemSummary): string {
  const left = it.progress
    ? it.runtime_s
      ? formatRemaining(it.runtime_s, it.progress)
      : `${Math.round(it.progress * 100)}% watched`
    : "";
  const code = it.kind === "episode" ? episodeTag(it) : "";
  return code && left ? `${code} · ${left}` : code || left;
}

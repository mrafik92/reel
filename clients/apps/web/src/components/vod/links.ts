import { href } from "../../lib/mode";
import type { DiscoverCard } from "../../api/types";
import type { DiscoverCardVod, VodId, VodKind } from "../../api/vod";

/** Detail page for an IPTV title. */
export function vodHref(c: { kind: VodKind; id: VodId }): string {
  return href(`/vod/${c.kind}/${encodeURIComponent(String(c.id))}`);
}

/**
 * Player for an IPTV movie or episode. `t` = resume second, `fromStart` ignores progress;
 * `series` lets the player label the episode and go back to its show.
 */
export function vodPlayHref(kind: "movie" | "episode", id: VodId, o: { t?: number; fromStart?: boolean; series?: VodId } = {}): string {
  const q: string[] = [];
  if (o.fromStart) q.push("start=0");
  else if (o.t && o.t > 5) q.push(`t=${Math.floor(o.t)}`);
  if (o.series != null) q.push(`series=${encodeURIComponent(String(o.series))}`);
  return href(`/vod/play/${kind}/${encodeURIComponent(String(id))}${q.length ? "?" + q.join("&") : ""}`);
}

/** The provider's copy of a TMDB title that isn't in the library: where "Play now (IPTV)" goes. */
export function vodTwin(c: DiscoverCard): string | undefined {
  const v = c as DiscoverCardVod;
  if (c.status === "available") return undefined;
  if (v.vod_movie_id != null) return vodPlayHref("movie", v.vod_movie_id);
  if (v.vod_series_id != null) return vodHref({ kind: "series", id: v.vod_series_id });
  return undefined;
}

/**
 * IPTV on-demand (Xtream VOD) — `/api/v1/vod/*`.
 *
 * Hand-written until the server's OpenAPI includes these. Same rules as the rest of the
 * client: HTTP by default, in-browser fixtures with `?mock=1` (code-split out of production).
 */
import type { CastMember, Page } from "@reel/ui";
import { MOCK } from "../lib/mode";
import { http } from "./http";
import { iptv } from "./iptv";
import type { DecideResponse, DiscoverCard, Quality, SearchAll } from "./types";

export type VodKind = "movie" | "series";
/** Provider ids are opaque to the client. */
export type VodId = number | string;

export interface VodCategory {
  id: VodId;
  name: string;
  count: number;
}

export interface VodCard {
  id: VodId;
  kind: VodKind;
  title: string;
  year?: number | null;
  poster_url?: string | null;
  rating?: number | null;
  genres?: string[] | null;
  tmdb_id?: number | null;
  category_id: VodId;
  /** The same title is in the local library: prefer local playback. */
  library_item_id?: number | null;
  /** 0..1 */
  progress?: number | null;
  played?: boolean | null;
}

export interface VodMovie extends VodCard {
  overview?: string | null;
  backdrop_url?: string | null;
  runtime_s?: number | null;
  director?: string | null;
  cast: CastMember[];
  trailer_youtube_id?: string | null;
  container?: string | null;
  /** Unix seconds */
  added_at?: number | null;
}

export interface VodEpisode {
  id: VodId;
  episode_no: number;
  title: string;
  overview?: string | null;
  still_url?: string | null;
  runtime_s?: number | null;
  progress?: number | null;
  played?: boolean | null;
}

export interface VodSeason {
  season_no: number;
  name?: string | null;
  poster_url?: string | null;
  episodes: VodEpisode[];
}

export interface VodSeries extends VodCard {
  overview?: string | null;
  backdrop_url?: string | null;
  cast: CastMember[];
  trailer_youtube_id?: string | null;
  seasons: VodSeason[];
}

export type VodSort = "added" | "rating" | "title" | "year";

export interface VodQuery {
  category?: VodId | null;
  q?: string;
  genre?: string;
  sort?: VodSort;
  cursor?: string | null;
  limit?: number;
}

/** Same shape as `/browse/lanes`; `see_all.query` is a `/vod/{movies|series}` query string. */
export interface VodLane {
  key: string;
  kind: string;
  title: string;
  subtitle?: string | null;
  items: VodCard[];
  see_all?: { query: string } | null;
}

export interface VodPlayBody {
  kind: "movie" | "episode";
  id: VodId;
  preset?: string | null;
  profile?: unknown;
  audio_index?: number | null;
  subtitle_index?: number | null;
  start_s?: number | null;
  /** Playback quality (#87); omitted = Auto. */
  quality?: Quality | null;
  /** Admins: stop other users' streams holding the provider (answer to a 409 `takeover: true`). */
  takeover?: boolean;
}

/**
 * Who holds the provider's single connection (429 body, when the server says).
 * `kind` tells the client which endpoint stops it.
 */
export interface VodHolder {
  user_id?: number | null;
  username?: string | null;
  /** The requesting user holds it (another screen of theirs). */
  mine?: boolean | null;
  session_id?: string | null;
  kind?: "live" | "vod" | string | null;
  title?: string | null;
  device?: string | null;
}

/** Discover cards and unified search, as extended by the VOD server round. */
export type DiscoverCardVod = DiscoverCard & { vod_movie_id?: VodId | null; vod_series_id?: VodId | null };
export type SearchAllVod = SearchAll & { vod?: VodCard[] | null };

export interface VodApi {
  categories(type: VodKind): Promise<VodCategory[]>;
  genres(type: VodKind): Promise<string[]>;
  list(type: VodKind, q: VodQuery): Promise<Page<VodCard>>;
  movie(id: VodId): Promise<VodMovie>;
  series(id: VodId): Promise<VodSeries>;
  lanes(type: VodKind): Promise<VodLane[]>;
  play(body: VodPlayBody): Promise<DecideResponse>;
  /** Free the provider connection held by another of the viewer's streams. */
  stopHolder(h: VodHolder): Promise<void>;
}

const plural = (t: VodKind) => (t === "movie" ? "movies" : "series");

const httpVod: VodApi = {
  categories: (type) => http.get("/vod/categories", { type }),
  genres: (type) => http.get("/vod/genres", { type }),
  list: (type, q) =>
    http.get(`/vod/${plural(type)}`, {
      category: q.category != null ? String(q.category) : undefined,
      q: q.q || undefined,
      genre: q.genre || undefined,
      sort: q.sort,
      cursor: q.cursor ?? undefined,
      limit: q.limit ?? 60,
    }),
  movie: (id) => http.get(`/vod/movies/${encodeURIComponent(String(id))}`),
  series: (id) => http.get(`/vod/series/${encodeURIComponent(String(id))}`),
  lanes: (type) => http.get("/vod/lanes", { type }),
  play: (body) => http.post("/vod/play", body),
  stopHolder: async (h) => {
    if (!h.session_id) return;
    if (h.kind === "live") await iptv.stop(h.session_id);
    else await http.del(`/playback/${encodeURIComponent(h.session_id)}`);
  },
};

let impl: Promise<VodApi> | undefined;
function backend(): Promise<VodApi> {
  if (!impl) impl = MOCK ? import("./mock/vod").then((m) => m.mockVod()) : Promise.resolve(httpVod);
  return impl;
}

// Short read cache so Back renders instantly (catalogues are big and change slowly).
const cache = new Map<string, { at: number; p: Promise<unknown> }>();
const TTL: Partial<Record<keyof VodApi, number>> = { categories: 10 * 60_000, genres: 2 * 60_000, list: 2 * 60_000, lanes: 5 * 60_000, movie: 60_000, series: 60_000 };

function wrap(): VodApi {
  const out: Record<string, unknown> = {};
  (Object.keys(httpVod) as (keyof VodApi)[]).forEach((k) => {
    out[k] = (...args: unknown[]) => {
      const run = () => backend().then((a) => (a[k] as unknown as (...x: unknown[]) => Promise<unknown>)(...args));
      const ttl = TTL[k];
      if (!ttl) return run();
      const key = `${k}:${JSON.stringify(args)}`;
      const hit = cache.get(key);
      if (hit && Date.now() - hit.at < ttl) return hit.p;
      const p = run();
      cache.set(key, { at: Date.now(), p });
      p.catch(() => cache.delete(key));
      return p;
    };
  });
  return out as unknown as VodApi;
}

export const vod: VodApi = wrap();

/** Progress changed (after playback): drop cached details and lanes. */
export function invalidateVod(): void {
  cache.forEach((_v, k) => {
    if (k.indexOf("movie:") === 0 || k.indexOf("series:") === 0 || k.indexOf("lanes:") === 0 || k.indexOf("list:") === 0) cache.delete(k);
  });
}

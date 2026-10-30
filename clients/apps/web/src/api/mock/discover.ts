import type { DiscoverCard } from "../types";
import { backdropArt, hash, posterArt, rng } from "./art";

/** Fictional "TMDB" catalogue for Discover in mock mode. */
const MOVIES = [
  "Dune Sea", "Comet Season", "The Glass Orchard", "Nightswimmers", "Cobalt Hour", "Iron Lullaby", "Paperweight",
  "The Ninth Lantern", "Satellite Hearts", "Wolfsbane Road", "The Quiet Engine", "Moth & Moon", "Tidewater",
  "Overlook 7", "The Brass Choir", "Halflight", "A Room of Clocks", "Saltwind", "Kingfisher", "The Last Funicular",
  "Neon Matins", "Granite", "Velvet Fog", "The Weather Thief", "Low Tide Hotel", "Juniper Falls",
];
const SHOWS = [
  "Lighthouse Keepers", "Static Bloom", "The Ferry Years", "North of Nowhere", "Kitchen Confidential Club",
  "The Archivists", "Dead Reckoning", "Signal & Noise", "Glasshouse", "The Orchardists", "Winter Circuit",
];
const BLURB = [
  "A small crew takes one last job that goes sideways in the most beautiful way.",
  "Two strangers share a sleeper carriage across a continent and a secret neither can keep.",
  "When the town's only cinema closes, its projectionist refuses to leave.",
  "A cartographer discovers an island that isn't on any map — including her own.",
  "An astronaut returns home to find the world moved on without her.",
  "A family of tightrope walkers stages a comeback in a city that forgot them.",
];

export interface TmdbEntry extends DiscoverCard {
  genres: string[];
  runtime_minutes?: number | null;
  seasons: number[];
  lists: ("trending" | "popular" | "upcoming")[];
}

export function buildTmdb(library: { tmdb_id: number; title: string; year?: number | null; media: "movie" | "tv"; poster?: string | null }[]): TmdbEntry[] {
  const r = rng(7);
  const out: TmdbEntry[] = [];
  const g = ["Drama", "Thriller", "Science Fiction", "Comedy", "Adventure", "Mystery", "Romance"];
  const mk = (title: string, media: "movie" | "tv", i: number): TmdbEntry => {
    const seed = hash(title) % 100000;
    const upcoming = media === "movie" && i % 6 === 5;
    return {
      tmdb_id: (media === "movie" ? 700000 : 800000) + i,
      media_type: media,
      title,
      year: upcoming ? 2027 : 1998 + Math.floor(r() * 29),
      overview: BLURB[i % BLURB.length]!,
      poster_url: posterArt(title, seed),
      backdrop_url: backdropArt(seed),
      vote_average: Math.round((6 + r() * 3) * 10) / 10,
      status: "none",
      progress: null,
      genres: [g[i % g.length]!, g[(i + 3) % g.length]!],
      runtime_minutes: media === "movie" ? 88 + Math.floor(r() * 60) : null,
      seasons: media === "tv" ? Array.from({ length: 1 + (i % 4) }, (_v, k) => k + 1) : [],
      lists: upcoming ? ["upcoming"] : i % 3 === 0 ? ["trending", "popular"] : i % 3 === 1 ? ["trending"] : ["popular"],
    };
  };
  MOVIES.forEach((t, i) => out.push(mk(t, "movie", i)));
  SHOWS.forEach((t, i) => out.push(mk(t, "tv", i)));
  // A few titles that already exist in the library (badge: In library / Partly).
  library.forEach((l, i) => {
    const e = mk(l.title, l.media, 100 + i);
    e.tmdb_id = l.tmdb_id;
    e.year = l.year ?? e.year;
    if (l.poster) e.poster_url = l.poster;
    e.status = l.media === "tv" && i % 2 === 1 ? "partially" : "available";
    e.lists = i % 2 ? ["popular"] : ["trending"];
    out.push(e);
  });
  return out;
}

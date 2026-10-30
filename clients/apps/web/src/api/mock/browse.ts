import type { ItemSummary, LibrarySort } from "@reel/ui";
import type { BrowseLane, DiscoverCard, GenreCount, LibraryFilters, PersonHit, SearchAll } from "../types";
import type { Db, Rec } from "./fixtures";
import { rng } from "./art";

/** Fixture summaries carry rating/runtime/genres, like the round-5 server. */
export function enrichSummaries(db: Db): void {
  db.byId.forEach((r) => {
    const k = r.summary.kind;
    if (k !== "movie" && k !== "show") return;
    r.summary.rating = r.rating || undefined;
    r.summary.runtime_s = r.runtime_s ?? null;
    r.summary.genres = r.genres;
  });
}

const titleKey = (s: string) => s.replace(/^(the|a|an)\s+/i, "").toLowerCase();

export function matches(r: Rec, f: LibraryFilters = {}): boolean {
  const s = r.summary;
  if (f.genre && f.genre.length && !f.genre.every((g) => r.genres.indexOf(g) >= 0)) return false;
  if (f.year_from != null && (s.year ?? 0) < f.year_from) return false;
  if (f.year_to != null && (s.year ?? 9999) > f.year_to) return false;
  if (f.min_rating != null && (r.rating ?? 0) < f.min_rating) return false;
  if (f.max_runtime_min != null && (!r.runtime_s || r.runtime_s / 60 > f.max_runtime_min)) return false;
  if (f.unwatched && s.played) return false;
  return true;
}

export function sortRecs(list: Rec[], sort: LibrarySort, seed = 1): Rec[] {
  const out = list.slice();
  const byTitle = (a: Rec, b: Rec) => (titleKey(a.summary.title) < titleKey(b.summary.title) ? -1 : 1);
  switch (sort) {
    case "added": return out.sort((a, b) => b.summary.added_at - a.summary.added_at);
    case "year": return out.sort((a, b) => (b.summary.year ?? 0) - (a.summary.year ?? 0) || byTitle(a, b));
    case "rating": return out.sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0) || byTitle(a, b));
    case "runtime": return out.sort((a, b) => (a.runtime_s ?? 1e9) - (b.runtime_s ?? 1e9) || byTitle(a, b));
    case "random": {
      const r = rng(seed);
      for (let i = out.length - 1; i > 0; i--) {
        const j = Math.floor(r() * (i + 1));
        const t = out[i]!;
        out[i] = out[j]!;
        out[j] = t;
      }
      return out;
    }
    default: return out.sort(byTitle);
  }
}

export function libraryRecs(db: Db, lib: number): Rec[] {
  return (db.byLibrary.get(lib) ?? []).map((id) => db.byId.get(id)!);
}

export function genreCounts(db: Db, lib?: number): GenreCount[] {
  const m = new Map<string, number>();
  const libs = lib != null ? [lib] : Array.from(db.byLibrary.keys());
  libs.forEach((l) => libraryRecs(db, l).forEach((r) => r.genres.forEach((g) => m.set(g, (m.get(g) ?? 0) + 1))));
  return Array.from(m.entries()).map(([name, count]) => ({ name, count })).sort((a, b) => b.count - a.count || (a.name < b.name ? -1 : 1));
}

const copy = (r: Rec): ItemSummary => ({ ...r.summary });

export function lanes(db: Db, lib: number): BrowseLane[] {
  const all = libraryRecs(db, lib);
  const shows = all.length > 0 && all[0]!.summary.kind === "show";
  const noun = shows ? "series" : "films";
  const out: BrowseLane[] = [];
  const unwatched = all.filter((r) => !r.summary.played);

  // Because you watched: most recent watched/in-progress title with a genre neighbour set.
  const anchor = all.find((r) => r.summary.progress) ?? all.find((r) => r.summary.played);
  if (anchor) {
    const g = anchor.genres[0];
    const similar = sortRecs(unwatched.filter((r) => r !== anchor && r.genres.some((x) => anchor.genres.indexOf(x) >= 0)), "rating").slice(0, 18);
    if (similar.length >= 3) {
      out.push({
        key: `byw:${anchor.summary.id}`, kind: "because_you_watched", title: `Because you watched ${anchor.summary.title}`,
        subtitle: anchor.genres.join(" · "), items: similar.map(copy),
        see_all: g ? { library_id: lib, query: `genre=${encodeURIComponent(g)}&unwatched=true&sort=rating` } : null,
      });
    }
  }
  const top = sortRecs(unwatched, "rating").slice(0, 20);
  if (top.length) out.push({ key: "top_unwatched", kind: "top_unwatched", title: "Top rated you haven't seen", items: top.map(copy), see_all: { library_id: lib, query: "sort=rating&unwatched=true" } });
  if (!shows) {
    const short = sortRecs(unwatched.filter((r) => r.runtime_s && r.runtime_s <= 100 * 60), "rating").slice(0, 20);
    if (short.length >= 3) out.push({ key: "short", kind: "short", title: "Under 100 minutes", subtitle: "For a weeknight", items: short.map(copy), see_all: { library_id: lib, query: "max_runtime_min=100&sort=runtime" } });
  }
  const fresh = sortRecs(all, "added").slice(0, 20);
  out.push({ key: "new", kind: "new", title: `New ${noun}`, items: fresh.map(copy), see_all: { library_id: lib, query: "sort=added" } });
  genreCounts(db, lib).slice(0, 4).forEach((g) => {
    const items = sortRecs(all.filter((r) => r.genres.indexOf(g.name) >= 0), "rating").slice(0, 20);
    if (items.length >= 3) out.push({ key: `genre:${g.name}`, kind: "genre", title: g.name, subtitle: `${g.count} ${noun}`, items: items.map(copy), see_all: { library_id: lib, query: `genre=${encodeURIComponent(g.name)}&sort=rating` } });
  });
  const again = sortRecs(all.filter((r) => r.summary.played), "rating").slice(0, 20);
  if (again.length >= 3) out.push({ key: "rediscover", kind: "rediscover", title: "Worth another watch", items: again.map(copy), see_all: { library_id: lib, query: "sort=rating" } });
  return out;
}

function fold(s: string): string {
  return s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");
}

export function searchAll(db: Db, q: string, limit: number, discover: DiscoverCard[]): SearchAll {
  const n = fold(q.trim());
  const empty: SearchAll = { library: [], episodes: [], people: [], discover: [] };
  if (!n) return empty;
  const rank = (t: string) => (fold(t).indexOf(n) === 0 ? 0 : 1);
  const library: ItemSummary[] = [];
  const episodes: ItemSummary[] = [];
  const people = new Map<number, PersonHit>();
  db.byId.forEach((r) => {
    const s = r.summary;
    const hit = fold(s.title).indexOf(n) >= 0;
    if ((s.kind === "movie" || s.kind === "show") && hit) library.push({ ...s });
    if (s.kind === "episode" && hit && !/^Episode \d+$/.test(s.title)) episodes.push({ ...s });
    if (s.kind === "movie" || s.kind === "show") {
      r.cast.forEach((c) => {
        if (!c.person_tmdb_id || fold(c.name).indexOf(n) < 0) return;
        const p = people.get(c.person_tmdb_id) ?? { tmdb_id: c.person_tmdb_id, name: c.name, photo_url: c.photo_url, known_for_department: "Acting", known_for: [], in_library_count: 0 };
        p.in_library_count++;
        if (p.known_for.length < 3) p.known_for.push(s.title);
        people.set(c.person_tmdb_id, p);
      });
    }
  });
  library.sort((a, b) => rank(a.title) - rank(b.title));
  episodes.sort((a, b) => rank(a.title) - rank(b.title));
  return {
    library: library.slice(0, limit),
    episodes: episodes.slice(0, Math.min(limit, 16)),
    people: Array.from(people.values()).sort((a, b) => rank(a.name) - rank(b.name) || b.in_library_count - a.in_library_count).slice(0, 16),
    discover: discover.filter((c) => c.status !== "available").slice(0, limit),
  };
}

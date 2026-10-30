import type { Library } from "@reel/ui";
import type { BrowseLane, LibraryFilters } from "../api";
import { href } from "./mode";

/**
 * Library URL state. Filter keys are the server's wire names so a lane's
 * `see_all.query` maps straight onto the page URL.
 */
export interface BrowseParams {
  lib?: string;
  /** "all" = the full grid even without filters. */
  view?: string;
  genre?: string;
  year_from?: string;
  year_to?: string;
  min_rating?: string;
  max_runtime_min?: string;
  unwatched?: string;
  sort?: string;
  seed?: string;
  [k: string]: string | undefined;
}

export const FILTER_KEYS = ["genre", "year_from", "year_to", "min_rating", "max_runtime_min", "unwatched"] as const;

const num = (v?: string): number | undefined => {
  if (v == null || v === "") return undefined;
  const n = Number(v);
  return isFinite(n) ? n : undefined;
};

/** The server wants whole numbers for years/minutes (anything else is a 400). */
const int = (v?: string): number | undefined => {
  const n = num(v);
  return n == null ? undefined : Math.round(n);
};

export function filtersFrom(p: BrowseParams): LibraryFilters {
  return {
    genre: p.genre ? p.genre.split(",").map((g) => g.trim()).filter(Boolean) : undefined,
    year_from: int(p.year_from),
    year_to: int(p.year_to),
    min_rating: num(p.min_rating),
    max_runtime_min: int(p.max_runtime_min),
    unwatched: p.unwatched === "true" || p.unwatched === "1",
  };
}

/** Full set of filter params (undefined clears a key in setSearchParams). */
export function paramsFrom(f: LibraryFilters): Partial<BrowseParams> {
  return {
    genre: f.genre && f.genre.length ? f.genre.join(",") : undefined,
    year_from: f.year_from != null ? String(f.year_from) : undefined,
    year_to: f.year_to != null ? String(f.year_to) : undefined,
    min_rating: f.min_rating != null ? String(f.min_rating) : undefined,
    max_runtime_min: f.max_runtime_min != null ? String(f.max_runtime_min) : undefined,
    unwatched: f.unwatched ? "true" : undefined,
  };
}

export function activeCount(f: LibraryFilters): number {
  return (f.genre?.length ?? 0) + (f.year_from != null || f.year_to != null ? 1 : 0) + (f.min_rating != null ? 1 : 0) +
    (f.max_runtime_min != null ? 1 : 0) + (f.unwatched ? 1 : 0);
}

export interface Decade {
  label: string;
  from?: number;
  to?: number;
}
export const DECADES: Decade[] = [
  { label: "2020s", from: 2020, to: 2029 },
  { label: "2010s", from: 2010, to: 2019 },
  { label: "2000s", from: 2000, to: 2009 },
  { label: "1990s", from: 1990, to: 1999 },
  { label: "1980s", from: 1980, to: 1989 },
  { label: "1970s", from: 1970, to: 1979 },
  { label: "Older", to: 1969 },
];
export function decadeLabel(f: LibraryFilters): string {
  if (f.year_from == null && f.year_to == null) return "";
  const d = DECADES.find((x) => x.from === f.year_from && x.to === f.year_to);
  if (d) return d.label;
  if (f.year_from != null && f.year_to != null) return `${f.year_from}–${f.year_to}`;
  return f.year_from != null ? `${f.year_from} or later` : `Before ${f.year_to! + 1}`;
}

/** "Tonight I have…" */
export const RUNTIMES: { label: string; value?: number }[] = [
  { label: "Any length" },
  { label: "90 min", value: 90 },
  { label: "2 hours", value: 120 },
];
export const RATINGS: { label: string; value?: number }[] = [
  { label: "Any" },
  { label: "6+", value: 6 },
  { label: "7+", value: 7 },
  { label: "8+", value: 8 },
];

/** Removable chips for the active filters. */
export function filterChips(f: LibraryFilters): { label: string; clear: Partial<LibraryFilters> }[] {
  const out: { label: string; clear: Partial<LibraryFilters> }[] = [];
  (f.genre ?? []).forEach((g) => out.push({ label: g, clear: { genre: (f.genre ?? []).filter((x) => x !== g) } }));
  const d = decadeLabel(f);
  if (d) out.push({ label: d, clear: { year_from: undefined, year_to: undefined } });
  if (f.min_rating != null) out.push({ label: `★ ${f.min_rating}+`, clear: { min_rating: undefined } });
  if (f.max_runtime_min != null) out.push({ label: `Under ${f.max_runtime_min >= 120 && f.max_runtime_min % 60 === 0 ? f.max_runtime_min / 60 + " h" : f.max_runtime_min + " min"}`, clear: { max_runtime_min: undefined } });
  if (f.unwatched) out.push({ label: "Unwatched", clear: { unwatched: false } });
  return out;
}

/** Plain-language summary for the picker ("unwatched thrillers under 90 min"). */
export function describePool(f: LibraryFilters, noun: string): string {
  const bits: string[] = [];
  if (f.unwatched) bits.push("unwatched");
  if (f.genre && f.genre.length) bits.push(f.genre.join(" + ").toLowerCase());
  bits.push(noun);
  const d = decadeLabel(f);
  if (d) bits.push(d === "Older" ? "from before 1970" : /s$/.test(d) ? `from the ${d}` : `from ${d}`);
  if (f.min_rating != null) bits.push(`rated ${f.min_rating}+`);
  if (f.max_runtime_min != null) bits.push(`under ${f.max_runtime_min} min`);
  return bits.join(" ");
}

/** Lane "See all" → the library grid with the lane's query applied. */
export function laneHref(lane: BrowseLane, libs: Library[]): string | undefined {
  const sa = lane.see_all;
  if (!sa) return undefined;
  const lib = libs.find((l) => l.id === sa.library_id);
  const base = lib?.kind === "shows" ? "/shows" : "/movies";
  const q = new URLSearchParams();
  q.set("lib", String(sa.library_id));
  new URLSearchParams((sa.query || "").replace(/^\?/, "")).forEach((v, k) => q.set(k, v));
  if (!q.has("view")) q.set("view", "all");
  return href(`${base}?${q.toString()}`);
}

export function genreHref(lib: Pick<Library, "id" | "kind">, genre: string): string {
  return href(`/${lib.kind === "shows" ? "shows" : "movies"}?lib=${lib.id}&genre=${encodeURIComponent(genre)}`);
}

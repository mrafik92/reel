import type {
  AudioStreamInfo, CastMember, Home, ItemDetail, ItemSummary, Library, MediaFile, SubtitleStreamInfo,
} from "@reel/ui";
import { backdropArt, hash, headshotArt, posterArt, rng } from "./art";

/** Served by the Vite dev server (see vite.config.ts → mockMedia). */
export const SAMPLE_VIDEO = "/mock-media/sample.webm";

interface MovieSeed {
  title: string;
  year: number;
  genres: string[];
  overview: string;
  runtime: number; // minutes
  rating: number;
  cr: string;
  tagline?: string;
}

const FEATURED_MOVIES: MovieSeed[] = [
  { title: "The Lantern Coast", year: 2025, genres: ["Drama", "Mystery"], runtime: 128, rating: 8.1, cr: "PG-13", tagline: "Every light is a promise",
    overview: "When the lighthouse on a remote Breton island goes dark for the first time in ninety years, a disgraced harbour pilot returns home to find the keeper missing and the village unwilling to say why." },
  { title: "Salt & Static", year: 2024, genres: ["Science Fiction", "Thriller"], runtime: 114, rating: 7.6, cr: "R",
    overview: "A night-shift radio engineer on a salt flat observatory starts receiving broadcasts from a station that was demolished in 1978 — and they describe events that haven't happened yet." },
  { title: "Paper Kingdoms", year: 2023, genres: ["Comedy", "Drama"], runtime: 101, rating: 7.2, cr: "PG-13",
    overview: "Two rival stationery shops on the same Lisbon street go to war over a single, very valuable fountain pen in this warm comedy about family businesses and stubborn love." },
  { title: "Northbound Nine", year: 2026, genres: ["Action", "Thriller"], runtime: 136, rating: 7.9, cr: "R", tagline: "One train. No stops.",
    overview: "A decommissioned overnight sleeper is hijacked somewhere above the Arctic Circle. The only person who knows every bolt of it is the retired conductor sleeping in carriage nine." },
  { title: "Small Hours", year: 2022, genres: ["Romance", "Drama"], runtime: 97, rating: 7.4, cr: "PG-13",
    overview: "Over a single sleepless night in Osaka, a jazz pianist and an insomniac translator walk the city and talk about everything except the reason neither can go home." },
  { title: "The Cartographer's Daughter", year: 2021, genres: ["Adventure", "Family"], runtime: 118, rating: 7.8, cr: "PG",
    overview: "Armed with her late father's unfinished atlas, a twelve-year-old sets out to chart the one valley he never reached, with a grumpy mule and a map that seems to redraw itself." },
  { title: "Velvet Engine", year: 2024, genres: ["Crime", "Drama"], runtime: 131, rating: 8.3, cr: "R",
    overview: "Detroit, 1971. A soul singer moonlighting as a getaway driver gets one last job — and a record deal — on the same Friday night." },
  { title: "Glasswing", year: 2025, genres: ["Animation", "Family"], runtime: 92, rating: 8.0, cr: "G",
    overview: "A shy dragonfly made of stained glass must cross a city of storms to relight the cathedral window she fell from." },
  { title: "Undertow", year: 2020, genres: ["Horror", "Mystery"], runtime: 104, rating: 6.9, cr: "R",
    overview: "A free-diver documenting a flooded mining town hears knocking from inside a sealed church — a hundred feet below the surface." },
  { title: "Meridian", year: 2019, genres: ["Science Fiction", "Drama"], runtime: 143, rating: 8.4, cr: "PG-13", tagline: "The long way home",
    overview: "Stranded on the dark side of a tidally locked planet, a botanist walks toward the terminator line where the sun never sets, carrying the last seeds of the colony." },
  { title: "Brass Hearts", year: 2023, genres: ["Music", "Comedy"], runtime: 108, rating: 7.1, cr: "PG",
    overview: "A failing colliery brass band enters a national competition with a conductor who can't read music and a tuba player who just got out of prison." },
  { title: "The Quiet Assignment", year: 2022, genres: ["Thriller", "Drama"], runtime: 122, rating: 7.5, cr: "R",
    overview: "A stenographer at a closed-door war crimes tribunal realises the testimony she is transcribing has been rehearsed — word for word — from a transcript she typed years ago." },
  { title: "Kite Season", year: 2018, genres: ["Family", "Drama"], runtime: 95, rating: 7.3, cr: "PG",
    overview: "In a windswept seaside town, three siblings build the biggest kite the world has ever seen to pull their grieving father out of his workshop." },
  { title: "Low Orbit", year: 2026, genres: ["Science Fiction", "Action"], runtime: 127, rating: 7.0, cr: "PG-13",
    overview: "A salvage crew racing to strip a dead space station discovers its last occupant is still aboard — and very much alive." },
  { title: "Wolves of Avalon Street", year: 2017, genres: ["Crime"], runtime: 139, rating: 7.7, cr: "R",
    overview: "Three generations of a Boston family run the last independent bar on a gentrifying street, and the developers have stopped asking nicely." },
  { title: "Honeyfield", year: 2021, genres: ["Romance", "Comedy"], runtime: 99, rating: 6.8, cr: "PG-13",
    overview: "A London food critic inherits half of a Welsh beekeeping farm. The other half belongs to the one man who ever gave her restaurant a bad review." },
];

const KIDS: MovieSeed[] = [
  { title: "Pip and the Moon Bakery", year: 2024, genres: ["Animation", "Family"], runtime: 84, rating: 7.6, cr: "G",
    overview: "A mouse who bakes bread for the moon has one night to fix the oven before the tides stop." },
  { title: "Captain Puddle", year: 2022, genres: ["Animation", "Comedy"], runtime: 79, rating: 6.9, cr: "G",
    overview: "The bravest rubber boot in the cupboard sets sail across the biggest puddle in town." },
  { title: "The Robot Who Collected Clouds", year: 2025, genres: ["Animation", "Adventure"], runtime: 88, rating: 7.8, cr: "PG",
    overview: "A weather robot breaks the rules to keep one small cloud as a pet, and a whole town loses its rain." },
];

const SHOWS: { title: string; year: number; genres: string[]; overview: string; seasons: number[]; cr: string; rating: number }[] = [
  { title: "Harbour Lights", year: 2022, genres: ["Drama", "Crime"], seasons: [8, 8, 10], cr: "TV-MA", rating: 8.5,
    overview: "Port authority investigators in Rotterdam untangle the smuggling routes that run through Europe's busiest harbour — and their own families." },
  { title: "The Long Quiet", year: 2024, genres: ["Science Fiction", "Mystery"], seasons: [6, 6], cr: "TV-14", rating: 8.2,
    overview: "Every radio in the world goes silent for nine minutes. Afterwards, one small Montana town can hear everything." },
  { title: "Kitchen Brigade", year: 2021, genres: ["Comedy"], seasons: [10, 10, 10, 8], cr: "TV-14", rating: 7.9,
    overview: "The chaotic staff of a failing Michelin-starred restaurant attempt to earn back their star — mostly by accident." },
  { title: "Ferryman", year: 2023, genres: ["Thriller", "Drama"], seasons: [8], cr: "TV-MA", rating: 7.7,
    overview: "A night ferry captain on the Baltic is paid to carry one passenger with no name across the water every Tuesday." },
  { title: "Field Notes", year: 2020, genres: ["Documentary"], seasons: [6, 6, 6], cr: "TV-G", rating: 8.8,
    overview: "Naturalists spend a year in a single square kilometre of wild land, filming everything that happens there." },
  { title: "Overtime", year: 2025, genres: ["Drama", "Sport"], seasons: [10], cr: "TV-14", rating: 7.4,
    overview: "A semi-pro ice hockey team in Finland's far north fights relegation and the closure of its only rink." },
];

const ADJ = ["Silent", "Crimson", "Hollow", "Last", "Burning", "Golden", "Distant", "Broken", "Midnight", "Paper", "Iron", "Velvet", "Wild", "Electric", "Winter", "Lucky", "Bitter", "Secret", "Painted", "Restless"];
const NOUN = ["Harbour", "Orchard", "Signal", "Garden", "Frontier", "Archive", "Mirror", "Season", "Canyon", "Parade", "Republic", "Lighthouse", "Carnival", "Garrison", "Atlas", "Rivers", "Summer", "Witness", "Machine", "Hotel"];
const SOLO = ["Ashfall", "Driftwood", "Kingsize", "Afterglow", "Monsoon", "Saltmarsh", "Flicker", "Overpass", "Tremor", "Nightjar", "Copperhead", "Quarry", "Halcyon", "Sundial", "Wildcard", "Bellwether"];
const GENRES = ["Drama", "Comedy", "Thriller", "Science Fiction", "Romance", "Crime", "Adventure", "Horror", "Mystery", "Action", "Documentary", "Music", "History", "Western"];
const ROLES = ["a retired cartographer", "an ambitious line cook", "a small-town sheriff", "two estranged sisters", "a night-shift nurse", "a disgraced chess prodigy", "a wedding photographer", "an aging stunt driver", "a young ferry pilot", "a lighthouse keeper"];
const PLOTS = [
  "is pulled into a decades-old disappearance that the whole town would rather forget",
  "has one week to pull off the most unlikely comeback of the season",
  "discovers a letter that rewrites everything they thought they knew about their family",
  "takes a job that turns out to be far more dangerous than advertised",
  "falls for the one person who stands in the way of their life's ambition",
  "must survive a single night in a city that has turned against them",
  "sets out across the country to return something that was never theirs",
];
const FIRST = ["Ada", "Marco", "Lena", "Théo", "Nadia", "Jonah", "Imani", "Felix", "Ines", "Tomás", "Priya", "Oskar", "Mei", "Rafael", "Sigrid", "Dev", "Clara", "Kofi", "Yara", "Anton", "Hana", "Luca"];
const LAST = ["Okafor", "Lindqvist", "Moreau", "Castellanos", "Hartley", "Nakamura", "Brennan", "Haddad", "Kowalski", "Ferreira", "Albrecht", "Osei", "Valdés", "Sørensen", "Whitlock", "Ivanova", "Delacroix", "Mbeki"];
const CHAR = ["Captain", "Detective", "Dr.", "Sister", "Mayor", "Agent", "Professor", "Coach"];

// ---------------------------------------------------------------------------

interface Rec {
  summary: ItemSummary;
  library_id: number;
  overview: string;
  genres: string[];
  runtime_s?: number;
  rating?: number;
  content_rating?: string;
  tmdb_id: number;
  cast: CastMember[];
  childIds: number[];
  seed: number;
}

export type { Rec };

export interface Db {
  libraries: Library[];
  byId: Map<number, Rec>;
  /** Top-level items (movies/shows) per library, in insertion order. */
  byLibrary: Map<number, number[]>;
}

/** Same shape as real image URLs; there is no server in mock mode, so these fail → placeholder. */
function missingImage(kind: "poster" | "backdrop", id: number): string {
  return `/api/v1/images/${kind}/${id}?w=${kind === "poster" ? 342 : 1280}`;
}

const NOW = Date.UTC(2026, 9, 30, 20, 0, 0);
const DAY = 86400_000;

/** Stable fake TMDB person id from a name (same person across films). */
export function personId(name: string): number {
  return 1_000_000 + (hash(name) % 900_000);
}

export function castFor(seed: number, n: number): CastMember[] {
  const r = rng(seed * 13 + 5);
  const out: CastMember[] = [];
  for (let i = 0; i < n; i++) {
    const name = `${FIRST[Math.floor(r() * FIRST.length)]} ${LAST[Math.floor(r() * LAST.length)]}`;
    const character = r() > 0.35
      ? `${FIRST[Math.floor(r() * FIRST.length)]} ${LAST[Math.floor(r() * LAST.length)]}`
      : `${CHAR[Math.floor(r() * CHAR.length)]} ${LAST[Math.floor(r() * LAST.length)]}`;
    // A few uncredited/NFO-only names have no TMDB id → not clickable.
    out.push({ name, character, photo_url: r() > 0.12 ? headshotArt(name) : null, person_tmdb_id: r() > 0.08 ? personId(name) : null });
  }
  return out;
}

export function buildDb(): Db {
  const libraries: Library[] = [
    { id: 1, name: "Films", kind: "movies", paths: ["/media/films"], item_count: 0 },
    { id: 2, name: "Series", kind: "shows", paths: ["/media/series"], item_count: 0 },
    { id: 3, name: "Kids & Family", kind: "movies", paths: ["/media/kids"], item_count: 0 },
  ];
  const byId = new Map<number, Rec>();
  const byLibrary = new Map<number, number[]>([[1, []], [2, []], [3, []]]);
  let nextId = 100;
  const r = rng(42);

  const addMovie = (lib: number, m: MovieSeed, addedDaysAgo: number) => {
    const id = nextId++;
    const seed = hash(m.title) % 100000;
    byId.set(id, {
      summary: {
        id, kind: "movie", title: m.title, year: m.year,
        poster_url: id % 29 === 7 ? missingImage("poster", id) : r() > 0.03 ? posterArt(m.title, seed, m.tagline) : null,
        backdrop_url: id % 37 === 11 ? missingImage("backdrop", id) : backdropArt(seed),
        played: false,
        progress: null,
        added_at: Math.floor((NOW - addedDaysAgo * DAY - Math.floor(r() * DAY)) / 1000),
      },
      library_id: lib, overview: m.overview, genres: m.genres, runtime_s: m.runtime * 60, rating: m.rating,
      content_rating: m.cr, tmdb_id: 500000 + id, cast: castFor(seed, 8 + Math.floor(r() * 6)), childIds: [], seed,
    });
    byLibrary.get(lib)!.push(id);
    return id;
  };

  FEATURED_MOVIES.forEach((m, i) => addMovie(1, m, i * 3 + 1));
  KIDS.forEach((m, i) => addMovie(3, m, i * 5 + 2));

  // Generated back catalogue.
  const used = new Set(FEATURED_MOVIES.map((m) => m.title));
  const gen = (lib: number, count: number, kids: boolean) => {
    for (let i = 0; i < count; i++) {
      let title = "";
      for (let tries = 0; tries < 20; tries++) {
        const p = r();
        title =
          p < 0.35 ? `The ${ADJ[Math.floor(r() * ADJ.length)]} ${NOUN[Math.floor(r() * NOUN.length)]}`
          : p < 0.6 ? SOLO[Math.floor(r() * SOLO.length)]! + (r() > 0.6 ? ` ${Math.floor(r() * 3) + 2}` : "")
          : p < 0.8 ? `${NOUN[Math.floor(r() * NOUN.length)]} of the ${ADJ[Math.floor(r() * ADJ.length)]}`
          : `${ADJ[Math.floor(r() * ADJ.length)]} ${NOUN[Math.floor(r() * NOUN.length)]}`;
        if (!used.has(title)) break;
      }
      used.add(title);
      const g1 = kids ? "Family" : GENRES[Math.floor(r() * GENRES.length)]!;
      const g2 = kids ? "Animation" : GENRES[Math.floor(r() * GENRES.length)]!;
      addMovie(lib, {
        title,
        year: kids ? 2005 + Math.floor(r() * 21) : 1962 + Math.floor(Math.pow(r(), 0.55) * 64),
        genres: g1 === g2 ? [g1] : [g1, g2],
        runtime: 82 + Math.floor(r() * 75),
        rating: Math.round((5.4 + r() * 3.5) * 10) / 10,
        cr: kids ? (r() > 0.5 ? "G" : "PG") : ["PG", "PG-13", "R", "R", "PG-13"][Math.floor(r() * 5)]!,
        overview: `${ROLES[Math.floor(r() * ROLES.length)]!.replace(/^./, (c) => c.toUpperCase())} ${PLOTS[Math.floor(r() * PLOTS.length)]}.`,
      }, 20 + Math.floor(r() * 900));
    }
  };
  gen(1, 232, false);
  gen(3, 33, true);

  // Shows → seasons → episodes.
  SHOWS.forEach((s, si) => {
    const showId = nextId++;
    const seed = hash(s.title) % 100000;
    const show: Rec = {
      summary: {
        id: showId, kind: "show", title: s.title, year: s.year,
        poster_url: posterArt(s.title, seed), backdrop_url: backdropArt(seed), played: false, progress: null,
        added_at: Math.floor((NOW - (si * 4 + 1) * DAY) / 1000),
      },
      library_id: 2, overview: s.overview, genres: s.genres, rating: s.rating, content_rating: s.cr,
      tmdb_id: 900000 + showId, cast: castFor(seed, 10), childIds: [], seed,
    };
    byId.set(showId, show);
    byLibrary.get(2)!.push(showId);
    s.seasons.forEach((epCount, k) => {
      const seasonNo = k + 1;
      const seasonId = nextId++;
      show.childIds.push(seasonId);
      const season: Rec = {
        summary: {
          id: seasonId, kind: "season", title: `Season ${seasonNo}`, year: s.year + k, season_no: seasonNo,
          parent_id: showId, poster_url: posterArt(`${s.title} ${seasonNo}`, seed + seasonNo, `Season ${seasonNo}`),
          backdrop_url: backdropArt(seed), played: false, added_at: show.summary.added_at,
        },
        library_id: 2, overview: s.overview, genres: s.genres, tmdb_id: 0, cast: [], childIds: [], seed: seed + seasonNo,
      };
      byId.set(seasonId, season);
      const finishedSeasons = Math.max(0, s.seasons.length - 1 - (si % 2));
      for (let e = 1; e <= epCount; e++) {
        const epId = nextId++;
        season.childIds.push(epId);
        const er = rng(seed + seasonNo * 100 + e);
        const words = [ADJ[Math.floor(er() * ADJ.length)], NOUN[Math.floor(er() * NOUN.length)]];
        // Episodes without metadata get server fallback titles ("Episode 7") and no still.
        const bare = si === 3 && seasonNo === 1 && e > 5;
        const title = bare ? `Episode ${e}` : er() > 0.5 ? `The ${words[0]} ${words[1]}` : `${words[1]}`;
        const played = seasonNo <= finishedSeasons || (seasonNo === finishedSeasons + 1 && e < 3);
        byId.set(epId, {
          summary: {
            id: epId, kind: "episode", title, season_no: seasonNo, episode_no: e, parent_id: seasonId,
            year: s.year + k, poster_url: null,
            backdrop_url: bare ? missingImage("backdrop", epId) : backdropArt(seed * 31 + seasonNo * 100 + e),
            played, progress: !played && seasonNo === finishedSeasons + 1 && e === 3 ? 0.38 + (si % 3) * 0.17 : null,
            added_at: Math.floor((NOW - (si * 4 + 1) * DAY + e * 3600_000) / 1000),
          },
          library_id: 2,
          overview: `${ROLES[Math.floor(er() * ROLES.length)]!.replace(/^./, (c) => c.toUpperCase())} ${PLOTS[Math.floor(er() * PLOTS.length)]}.`,
          genres: s.genres, runtime_s: (42 + Math.floor(er() * 18)) * 60, rating: Math.round((7 + er() * 2) * 10) / 10,
          content_rating: s.cr, tmdb_id: 0, cast: [], childIds: [], seed: seed + e,
        });
      }
    });
  });

  // A few films in progress / watched.
  const films = byLibrary.get(1)!;
  const progress: [number, number][] = [[0, 0.62], [3, 0.18], [6, 0.84], [9, 0.41]];
  for (const [i, p] of progress) byId.get(films[i]!)!.summary.progress = p;
  for (const i of [1, 4, 10, 12, 20, 21, 33]) byId.get(films[i]!)!.summary.played = true;

  for (const lib of libraries) lib.item_count = byLibrary.get(lib.id)!.length;
  return { libraries, byId, byLibrary };
}

// ---------------------------------------------------------------------------

const AUDIO_SETS: AudioStreamInfo[][] = [
  [
    { index: 1, codec: "eac3", channels: 6, language: "eng", title: null, default: true },
    { index: 2, codec: "aac", channels: 2, language: "eng", title: "Director's commentary", default: false },
    { index: 3, codec: "ac3", channels: 6, language: "fre", title: null, default: false },
  ],
  [
    { index: 1, codec: "truehd", channels: 8, language: "eng", title: "Atmos", default: true },
    { index: 2, codec: "ac3", channels: 6, language: "eng", title: null, default: false },
    { index: 3, codec: "eac3", channels: 6, language: "ger", title: null, default: false },
    { index: 4, codec: "eac3", channels: 6, language: "spa", title: null, default: false },
  ],
  [{ index: 1, codec: "aac", channels: 2, language: "eng", title: null, default: true }],
];
const SUB_SETS: SubtitleStreamInfo[][] = [
  [
    { index: 4, codec: "subrip", language: "eng", title: null, forced: false, external: false },
    { index: 5, codec: "subrip", language: "eng", title: "SDH", forced: false, external: false },
    { index: 6, codec: "hdmv_pgs_subtitle", language: "fre", title: null, forced: false, external: false },
    { index: 7, codec: "subrip", language: "spa", title: null, forced: false, external: true },
  ],
  [
    { index: 5, codec: "hdmv_pgs_subtitle", language: "eng", title: null, forced: false, external: false },
    { index: 6, codec: "hdmv_pgs_subtitle", language: "eng", title: "Forced", forced: true, external: false },
  ],
  [],
];

function filesFor(rec: Rec): MediaFile[] {
  if (rec.summary.kind !== "movie" && rec.summary.kind !== "episode") return [];
  const r = rng(rec.seed * 3 + 11);
  const dur = rec.runtime_s ?? 3600;
  // The mock stream is a 90s sample; report its real length so resume/seek line up.
  const SAMPLE_S = 90;
  const variant = rec.summary.kind === "episode" ? 2 : Math.floor(r() * 3);
  const uhd = variant === 1;
  const main: MediaFile = {
    id: rec.summary.id * 10 + 1,
    container: uhd ? "mkv" : variant === 0 ? "mkv" : "mp4",
    duration_s: SAMPLE_S,
    size: Math.round(dur * (uhd ? 6.2e6 : variant === 0 ? 1.4e6 : 0.45e6)),
    url: SAMPLE_VIDEO,
    video: uhd
      ? { codec: "hevc", width: 3840, height: 2160, hdr: r() > 0.5 ? "dv" : "hdr10", bit_depth: 10 }
      : { codec: variant === 0 ? "hevc" : "h264", width: 1920, height: variant === 0 ? 804 : 1080, hdr: "none", bit_depth: variant === 0 ? 10 : 8 },
    audio: AUDIO_SETS[variant]!,
    subtitles: rec.summary.kind === "episode" ? SUB_SETS[0]!.slice(0, 2) : SUB_SETS[variant]!,
  };
  if (!uhd || r() > 0.5) return [main];
  // 4K titles sometimes carry a 1080p version too.
  return [
    main,
    {
      id: rec.summary.id * 10 + 2, container: "mp4", duration_s: SAMPLE_S, size: Math.round(dur * 1.1e6), url: SAMPLE_VIDEO,
      video: { codec: "h264", width: 1920, height: 1080, hdr: "none", bit_depth: 8 },
      audio: AUDIO_SETS[2]!, subtitles: SUB_SETS[0]!,
    },
  ];
}

export function detail(db: Db, id: number): ItemDetail | undefined {
  const rec = db.byId.get(id);
  if (!rec) return undefined;
  let cast = rec.cast;
  if (!cast.length && rec.summary.kind !== "show") {
    // Episodes & seasons inherit the show cast.
    let p = rec.summary.parent_id != null ? db.byId.get(rec.summary.parent_id) : undefined;
    while (p && !p.cast.length && p.summary.parent_id != null) p = db.byId.get(p.summary.parent_id);
    cast = p?.cast ?? [];
  }
  return {
    ...rec.summary,
    overview: rec.overview,
    runtime_s: rec.runtime_s ?? null,
    rating: rec.rating ?? null,
    content_rating: rec.content_rating ?? null,
    genres: rec.genres,
    tmdb_id: rec.tmdb_id || null,
    cast,
    children_count: rec.childIds.length,
    files: multiEpisode(db, rec) ?? filesFor(rec),
  };
}

/** Mock a multi-episode file: Kitchen Brigade S1E1+E2 live in one `S01E01E02` file. */
function multiEpisode(db: Db, rec: Rec): MediaFile[] | undefined {
  const s = rec.summary;
  if (s.kind !== "episode" || s.season_no !== 1 || (s.episode_no !== 1 && s.episode_no !== 2)) return undefined;
  const season = s.parent_id != null ? db.byId.get(s.parent_id) : undefined;
  const show = season?.summary.parent_id != null ? db.byId.get(season.summary.parent_id) : undefined;
  if (!season || show?.summary.title !== "Kitchen Brigade") return undefined;
  const ids = season.childIds.slice(0, 2);
  const first = db.byId.get(ids[0]!)!;
  const f = filesFor(first)[0]!;
  return [{ ...f, id: ids[0]! * 10 + 1, item_ids: ids }];
}

export function home(db: Db): Home {
  const all = Array.from(db.byId.values());
  const cw = all
    .filter((r) => (r.summary.kind === "movie" || r.summary.kind === "episode") && r.summary.progress)
    .map((r) => r.summary);
  const recent = (lib: number) =>
    db.byLibrary
      .get(lib)!
      .map((id) => db.byId.get(id)!.summary)
      .sort((a, b) => (a.added_at < b.added_at ? 1 : -1));
  const recentlyAdded = recent(1).slice(0, 8).concat(recent(2).slice(0, 4), recent(3).slice(0, 3))
    .sort((a, b) => (a.added_at < b.added_at ? 1 : -1));
  const topRated = (lib: number) =>
    db.byLibrary
      .get(lib)!
      .map((id) => db.byId.get(id)!)
      .sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0))
      .map((r) => r.summary);
  return {
    continue_watching: cw,
    recently_added: recentlyAdded,
    rows: [
      { library_id: 2, title: "Series", items: recent(2) },
      { library_id: 1, title: "Films", items: topRated(1).slice(0, 20) },
      { library_id: 3, title: "Kids & Family", items: recent(3).slice(0, 20) },
    ],
  };
}

/** Mock "import": a requested title lands in the library once its download completes. */
export function importTitle(
  db: Db,
  t: { title: string; year?: number | null; tmdb_id: number; media_type: "movie" | "tv"; overview?: string | null; genres?: string[] },
): number {
  let max = 0;
  db.byId.forEach((_v, k) => (max = Math.max(max, k)));
  const id = max + 1;
  const seed = hash(t.title) % 100000;
  const lib = t.media_type === "tv" ? 2 : 1;
  const nowS = Math.floor(Date.now() / 1000);
  db.byId.set(id, {
    summary: {
      id, kind: t.media_type === "tv" ? "show" : "movie", title: t.title, year: t.year ?? null,
      poster_url: posterArt(t.title, seed), backdrop_url: backdropArt(seed), played: false, progress: null, added_at: nowS,
    },
    library_id: lib, overview: t.overview ?? "", genres: t.genres ?? [], runtime_s: 6600, rating: 7.4,
    content_rating: "PG-13", tmdb_id: t.tmdb_id, cast: castFor(seed, 8), childIds: [], seed,
  });
  db.byLibrary.get(lib)!.push(id);
  const l = db.libraries.find((x) => x.id === lib);
  if (l) l.item_count++;
  return id;
}

/** tmdb_id → library item id (for availability badges). */
export function tmdbIndex(db: Db): Map<number, number> {
  const m = new Map<number, number>();
  db.byId.forEach((r, id) => {
    if (r.tmdb_id && (r.summary.kind === "movie" || r.summary.kind === "show")) m.set(r.tmdb_id, id);
  });
  return m;
}

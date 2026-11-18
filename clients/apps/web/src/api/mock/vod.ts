/**
 * Mock IPTV on-demand catalogue (`?mock=1`): a big, messy, mostly-Arabic + international
 * Xtream-style catalogue — 100+ provider buckets, thousands of titles, patchy metadata.
 */
import type { CastMember } from "@reel/ui";
import { mockQuality } from "./select";
import { ApiError } from "../http";
import type { DecideResponse } from "../types";
import type {
  VodApi, VodCard, VodCategory, VodEpisode, VodHolder, VodId, VodKind, VodLane, VodMovie, VodQuery, VodSeason, VodSeries,
} from "../vod";
import { backdropArt, hash, posterArt, rng } from "./art";
import { buildDb, castFor, SAMPLE_VIDEO } from "./fixtures";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const latency = () => sleep(140 + Math.random() * 220);
const NOW_S = Math.floor(Date.UTC(2026, 9, 30, 20, 0, 0) / 1000);

const fold = (s: string) => s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");

// ---------------------------------------------------------------------------
// Provider buckets

/** [name, weight, language, genre hint] */
type CatSeed = [string, number, "ar" | "en" | "xx", string?];

const MOVIE_CATS: CatSeed[] = [
  ["AR | New Releases 2026", 60, "ar"], ["AR | Egyptian Movies", 120, "ar"], ["AR | Egyptian Classics", 80, "ar", "Drama"],
  ["AR | Gulf Movies", 40, "ar"], ["AR | Syrian Movies", 30, "ar", "Drama"], ["AR | Lebanese Movies", 28, "ar"],
  ["AR | Moroccan Movies", 26, "ar"], ["AR | Tunisian Movies", 18, "ar"], ["AR | Iraqi Movies", 14, "ar"],
  ["AR | Comedy", 50, "ar", "Comedy"], ["AR | Action", 44, "ar", "Action"], ["AR | Horror", 16, "ar", "Horror"],
  ["AR | Ramadan Specials", 20, "ar", "Family"], ["AR | Plays مسرحيات", 36, "ar", "Comedy"], ["AR | Dubbed Movies", 40, "ar"],
  ["AR | Kids Dubbed", 34, "ar", "Animation"], ["AR | Documentary", 18, "ar", "Documentary"],
  ["EN | New Releases", 90, "en"], ["EN | 4K UHD", 60, "en"], ["EN | Action", 80, "en", "Action"], ["EN | Comedy", 70, "en", "Comedy"],
  ["EN | Drama", 70, "en", "Drama"], ["EN | Horror", 50, "en", "Horror"], ["EN | Thriller", 56, "en", "Thriller"],
  ["EN | Sci-Fi", 44, "en", "Science Fiction"], ["EN | Romance", 36, "en", "Romance"], ["EN | Documentary", 30, "en", "Documentary"],
  ["EN | Animation", 40, "en", "Animation"], ["EN | Family", 30, "en", "Family"], ["EN | Crime", 34, "en", "Crime"],
  ["EN | War", 18, "en", "War"], ["EN | Western", 14, "en", "Western"], ["EN | Oscar Winners", 24, "en", "Drama"],
  ["EN | Christmas", 16, "en", "Family"], ["EN | Stand-up Comedy", 20, "en", "Comedy"],
  ["NETFLIX | Movies", 70, "en"], ["DISNEY+ | Movies", 40, "en", "Family"], ["APPLE TV+ | Movies", 20, "en"],
  ["PRIME | Movies", 34, "en"], ["HBO | Movies", 22, "en"], ["SHAHID | أفلام", 40, "ar"], ["OSN+ | Movies", 24, "en"],
  ["BOX OFFICE | Cinema", 20, "en"], ["MULTI-AUDIO | Movies", 26, "en"], ["4K | HDR", 30, "en"], ["3D | Movies", 8, "en"],
  ["FR | Films", 40, "xx"], ["FR | Nouveautés", 20, "xx"], ["DE | Filme", 30, "xx"], ["IT | Film", 22, "xx"],
  ["ES | Películas", 30, "xx"], ["LAT | Películas", 26, "xx"], ["PT | Filmes", 16, "xx"], ["NL | Films", 12, "xx"],
  ["TR | Türk Filmleri", 40, "xx"], ["TR | Yeni Filmler", 16, "xx"], ["IN | Bollywood", 60, "xx"], ["IN | Tollywood", 30, "xx"],
  ["IN | Punjabi", 14, "xx"], ["PK | Pakistani", 12, "xx"], ["IR | Persian Movies", 18, "xx"], ["KU | Kurdish", 8, "xx"],
  ["KR | Korean Movies", 30, "xx"], ["JP | Anime Movies", 26, "xx", "Animation"], ["CN | Chinese Movies", 20, "xx"],
  ["PH | Filipino", 10, "xx"], ["AF | African Movies", 14, "xx"], ["SE | Filmer", 8, "xx"], ["PL | Filmy", 10, "xx"],
  ["RU | Фильмы", 18, "xx"], ["GR | Ταινίες", 8, "xx"], ["AL | Filma", 6, "xx"], ["RO | Filme", 8, "xx"],
  ["KIDS | Cartoons", 50, "en", "Animation"], ["KIDS | Pixar & Friends", 24, "en", "Animation"], ["KIDS | Arabic Cartoons", 30, "ar", "Animation"],
  ["SPORT | Documentaries", 14, "en", "Documentary"], ["MUSIC | Concerts", 12, "en", "Music"], ["ISLAMIC | Films", 14, "ar", "History"],
  ["COLLECTION | Spy Sagas", 20, "en", "Action"], ["COLLECTION | Car Chases", 12, "en", "Action"], ["COLLECTION | Wizarding", 10, "en", "Fantasy"],
  ["COLLECTION | Space Operas", 14, "en", "Science Fiction"], ["COLLECTION | Heist", 12, "en", "Crime"], ["COLLECTION | Monsters", 10, "en", "Horror"],
  ["COLLECTION | Adel Emam", 30, "ar", "Comedy"], ["COLLECTION | Ahmed Helmy", 16, "ar", "Comedy"], ["COLLECTION | Faten Hamama", 18, "ar", "Drama"],
  ["COLLECTION | Youssef Chahine", 12, "ar", "Drama"], ["COLLECTION | Soad Hosny", 14, "ar", "Romance"], ["COLLECTION | Black & White", 24, "ar", "Drama"],
  ["VIP | Premium 1080p", 30, "en"], ["VIP | Premium Arabic", 30, "ar"], ["ADULT ANIMATION", 8, "en", "Animation"],
  ["EN | Biography", 16, "en", "History"], ["EN | Music", 12, "en", "Music"], ["EN | Mystery", 20, "en", "Mystery"],
  ["EN | Fantasy", 20, "en", "Fantasy"], ["EN | Sports", 12, "en", "Sport"], ["EN | Classics", 26, "en", "Drama"],
  ["AR | Algerian Movies", 12, "ar"], ["AR | Sudanese Movies", 6, "ar"], ["AR | Jordanian Movies", 8, "ar"], ["AR | Palestinian Cinema", 10, "ar", "Drama"],
];

const SERIES_CATS: CatSeed[] = [
  ["AR | Ramadan 2026", 40, "ar"], ["AR | Ramadan 2025", 36, "ar"], ["AR | Egyptian Series", 50, "ar"], ["AR | Syrian Series", 30, "ar", "Drama"],
  ["AR | Gulf Series", 24, "ar"], ["AR | Lebanese Series", 16, "ar"], ["AR | Moroccan Series", 12, "ar"], ["AR | Old Classics", 20, "ar", "Drama"],
  ["AR | Sitcoms", 16, "ar", "Comedy"], ["AR | Kids Series", 16, "ar", "Animation"], ["SHAHID | مسلسلات", 30, "ar"],
  ["TR | Turkish Series (Arabic Dubbed)", 40, "ar", "Drama"], ["TR | Türk Dizileri", 24, "xx", "Drama"], ["IN | Hindi Series (Arabic Dubbed)", 14, "ar"],
  ["KR | K-Drama", 30, "xx", "Romance"], ["NETFLIX | Series", 50, "en"], ["HBO | Series", 26, "en"], ["APPLE TV+ | Series", 16, "en"],
  ["DISNEY+ | Series", 20, "en"], ["PRIME | Series", 20, "en"], ["EN | Drama", 30, "en", "Drama"], ["EN | Comedy", 30, "en", "Comedy"],
  ["EN | Crime", 24, "en", "Crime"], ["EN | Sci-Fi", 16, "en", "Science Fiction"], ["EN | Reality", 14, "en", "Reality"],
  ["EN | Documentary Series", 20, "en", "Documentary"], ["EN | Anime", 24, "en", "Animation"], ["KIDS | Cartoons", 30, "en", "Animation"],
  ["FR | Séries", 14, "xx"], ["ES | Series", 14, "xx"], ["DE | Serien", 10, "xx"], ["LAT | Novelas", 16, "xx", "Romance"],
];

// ---------------------------------------------------------------------------
// Title generators

const AR_A = ["ليل", "بيت", "طريق", "سر", "قلب", "ظل", "نجمة", "صيف", "شارع", "مدينة", "حكاية", "رسائل", "أيام", "زمن", "عودة", "بحر", "سوق", "باب", "حلم", "وعد", "أبناء", "نساء", "رجال", "ضحكة"];
const AR_B = ["المدينة", "الرمال", "الشمال", "القمر", "الذهب", "الصحراء", "الإسكندرية", "النيل", "الضباب", "العيد", "الحب", "الغائب", "الورد", "الجزيرة", "البحر", "الماضي", "الحارة", "الأمل", "الليل", "الشتاء", "الجبل", "الريح"];
const AR_LATIN = ["Kira w El Gen", "El Kebeer", "Bab El Shams", "Welad Rizk", "Sahar El Layali", "Hepta", "El Fil El Azraq", "Teer Enta", "Sherbet Loz", "Hassan w Morcos", "El Mamar", "Laylat El Eid", "Bel Arabi", "Khaly Balak", "Al Hayba", "El Asleyeen"];
const AR_OVERVIEW = [
  "حين يعود سليم إلى حارته القديمة بعد عشرين عاماً، يكتشف أن كل شيء تغيّر إلا سرّاً واحداً لم يجرؤ أحد على البوح به.",
  "عائلة بسيطة في الإسكندرية تجد نفسها في قلب صراع على ميراث لم يكن في الحسبان، في كوميديا دافئة عن الطمع والمحبة.",
  "ضابط شاب يلاحق عصابة تهريب على طول الحدود، ليكتشف أن أقرب الناس إليه يعرف أكثر مما يقول.",
  "قصة حب تبدأ في ليلة العيد وتمتد عبر ثلاثين عاماً من التحولات في مدينة لا تنام.",
  "مطربة شعبية تحاول العودة إلى المسرح بعد غياب طويل، بينما تطاردها فضيحة قديمة.",
];
const EN_ADJ = ["Silent", "Crimson", "Hollow", "Last", "Burning", "Golden", "Distant", "Broken", "Midnight", "Paper", "Iron", "Velvet", "Wild", "Electric", "Winter", "Lucky", "Bitter", "Secret", "Painted", "Restless", "Northern", "Savage"];
const EN_NOUN = ["Harbour", "Orchard", "Signal", "Garden", "Frontier", "Archive", "Mirror", "Season", "Canyon", "Parade", "Republic", "Protocol", "Carnival", "Garrison", "Atlas", "Rivers", "Summer", "Witness", "Machine", "Hotel", "Empire", "Cartel"];
const EN_SOLO = ["Ashfall", "Driftwood", "Afterglow", "Monsoon", "Saltmarsh", "Flicker", "Overpass", "Tremor", "Nightjar", "Copperhead", "Quarry", "Halcyon", "Sundial", "Wildcard", "Bellwether", "Firebreak", "Undertow", "Riptide"];
const XX = ["La Nuit de Marseille", "Der Lange Winter", "Il Silenzio", "La Casa del Mar", "Amor en Llamas", "Kara Sevda", "Dil Se Dil Tak", "Bahar Ki Raat", "Seoul Station", "Tokyo Drift Club", "Les Enfants du Port", "Das Boot Haus", "Sotto il Sole", "El Último Tren", "O Mar Azul", "Yeni Hayat", "Pyaar Ka Safar", "Sarab", "Moskva Noch", "Jeju Blues", "Kaze no Machi", "Babaji", "Ayla", "Söz"];
const EN_OVERVIEW = [
  "A small crew takes one last job that goes sideways in the most beautiful way.",
  "Two strangers share a sleeper carriage across a continent and a secret neither can keep.",
  "When the town's only cinema closes, its projectionist refuses to leave.",
  "An ex-soldier turned bodyguard is hired to protect the one witness everyone wants silenced.",
  "A family reunion on a remote island turns deadly when the ferry stops running.",
  "A detective with a perfect record takes the case that ended her father's career.",
];
const GENRES = ["Drama", "Comedy", "Action", "Thriller", "Romance", "Crime", "Horror", "Family", "Science Fiction", "Adventure", "Mystery", "Animation", "History", "Documentary"];
const AR_NUM = ["٢", "٣"];

/** Discover ("TMDB") titles that the provider also carries → Play now (IPTV) on Discover. */
const DISCOVER_MOVIES: [string, number, number][] = [["Tidewater", 700012, 2014], ["Halflight", 700015, 2021], ["Saltwind", 700017, 2009], ["Kingfisher", 700018, 2016], ["Nightswimmers", 700003, 2019], ["Velvet Fog", 700022, 2012]];
const DISCOVER_SHOWS: [string, number, number][] = [["Lighthouse Keepers", 800000, 2017], ["Signal & Noise", 800007, 2022], ["Glasshouse", 800008, 2020]];

interface MovieRec extends VodMovie {
  overview_ar?: boolean;
}

export interface VodDb {
  movieCats: VodCategory[];
  seriesCats: VodCategory[];
  movies: MovieRec[];
  series: VodSeries[];
  movieById: Map<string, MovieRec>;
  seriesById: Map<string, VodSeries>;
  episodeById: Map<string, { series: VodSeries; season: VodSeason; ep: VodEpisode }>;
}

function build(): VodDb {
  const r = rng(2026);
  const lib = buildDb();
  const libFilms = (lib.byLibrary.get(1) ?? []).slice(0, 8).map((id) => lib.byId.get(id)!);
  const libShows = (lib.byLibrary.get(2) ?? []).slice(0, 2).map((id) => lib.byId.get(id)!);
  const pick = <T,>(a: T[]): T => a[Math.floor(r() * a.length)]!;
  const used = new Set<string>();

  const titleFor = (lang: CatSeed[2]): string => {
    let t = "";
    for (let i = 0; i < 30; i++) {
      const p = r();
      if (lang === "ar") t = p < 0.62 ? `${pick(AR_A)} ${pick(AR_B)}${r() > 0.85 ? " " + pick(AR_NUM) : ""}` : pick(AR_LATIN) + (r() > 0.6 ? ` ${Math.floor(r() * 3) + 2}` : "");
      else if (lang === "xx") t = p < 0.5 ? pick(XX) + (r() > 0.55 ? ` ${Math.floor(r() * 4) + 2}` : "") : `${pick(EN_SOLO)} ${pick(["Express", "Nights", "Diaries", "Blues", "Story", "Affair"])}`;
      else t = p < 0.4 ? `The ${pick(EN_ADJ)} ${pick(EN_NOUN)}` : p < 0.7 ? pick(EN_SOLO) + (r() > 0.6 ? ` ${Math.floor(r() * 3) + 2}` : "") : `${pick(EN_NOUN)} of the ${pick(EN_ADJ)}`;
      if (!used.has(t)) break;
    }
    // Providers list the same title in several buckets anyway; duplicates are realistic.
    used.add(t);
    return t;
  };

  const makeCats = (seeds: CatSeed[], base: number): VodCategory[] => seeds.map((s, i) => ({ id: base + i, name: s[0], count: 0 }));
  const movieCats = makeCats(MOVIE_CATS, 100);
  const seriesCats = makeCats(SERIES_CATS, 500);

  // ---- Movies
  const movies: MovieRec[] = [];
  let mid = 10001;
  const addMovie = (catIdx: number, o: Partial<MovieRec> = {}): MovieRec => {
    const seed = MOVIE_CATS[catIdx]!;
    const cat = movieCats[catIdx]!;
    const lang = seed[2];
    const title = o.title ?? titleFor(lang);
    const s = hash(title) % 100000;
    const g1 = seed[3] ?? pick(GENRES);
    const g2 = pick(GENRES);
    const hasMeta = r() > 0.25;
    const m: MovieRec = {
      id: mid++, kind: "movie", title, year: 1958 + Math.floor(Math.pow(r(), 0.45) * 69),
      poster_url: r() > 0.07 ? posterArt(title, s) : null,
      rating: hasMeta && r() > 0.15 ? Math.round((4.6 + r() * 4.2) * 10) / 10 : null,
      genres: g1 === g2 ? [g1] : [g1, g2],
      tmdb_id: hasMeta ? 900000 + mid : null,
      category_id: cat.id,
      progress: null, played: false,
      overview: hasMeta || r() > 0.5 ? (lang === "ar" && r() > 0.3 ? pick(AR_OVERVIEW) : pick(EN_OVERVIEW)) : null,
      backdrop_url: r() > 0.2 ? backdropArt(s) : null,
      runtime_s: r() > 0.1 ? (78 + Math.floor(r() * 75)) * 60 : null,
      director: hasMeta ? `${pick(["Marwan", "Yousry", "Hala", "Sherif", "Ines", "Tomás", "Nadia", "Kofi"])} ${pick(["Hamed", "Nasrallah", "Lotfy", "Arafa", "Moreau", "Okafor", "Haddad"])}` : null,
      cast: hasMeta ? castFor(s, 6 + Math.floor(r() * 6)) : r() > 0.5 ? castFor(s, 3).map((c): CastMember => ({ name: c.name, character: null, photo_url: null, person_tmdb_id: null })) : [],
      trailer_youtube_id: hasMeta && r() > 0.5 ? "aqz-KE-bpKQ" : null,
      container: pick(["mkv", "mp4", "mp4", "avi", "ts"]),
      added_at: NOW_S - Math.floor(Math.pow(r(), 1.8) * 420 * 86400),
      ...o,
    };
    if (m.year && m.year > 2026) m.year = 2026;
    cat.count++;
    movies.push(m);
    return m;
  };
  const totalW = MOVIE_CATS.reduce((a, c) => a + c[1], 0);
  const N_MOVIES = 2600;
  MOVIE_CATS.forEach((c, i) => {
    const n = Math.max(3, Math.round((c[1] / totalW) * N_MOVIES));
    for (let k = 0; k < n; k++) addMovie(i);
  });
  // The same films the library has (badge: In library) — spread over a few buckets.
  libFilms.forEach((rec, i) => {
    const m = addMovie([17, 18, 35, 19][i % 4]!, {
      title: rec.summary.title, year: rec.summary.year, tmdb_id: rec.tmdb_id, library_item_id: rec.summary.id,
      poster_url: rec.summary.poster_url, backdrop_url: rec.summary.backdrop_url, overview: rec.overview,
      rating: rec.rating ?? null, genres: rec.genres, runtime_s: rec.runtime_s ?? null, cast: rec.cast,
      added_at: NOW_S - (i + 1) * 86400 * 2,
    });
    used.add(m.title);
  });
  DISCOVER_MOVIES.forEach(([t, tmdb, year], i) => {
    addMovie([17, 35, 18, 36, 19, 20][i]!, {
      title: t, tmdb_id: tmdb, year, poster_url: posterArt(t, hash(t) % 100000), added_at: NOW_S - (i + 3) * 3600 * 7,
    });
  });
  // Someone else is on the provider whenever this one is opened (mock 429, other user).
  addMovie(17, { title: "Signal Lost", year: 2025, rating: 6.4, added_at: NOW_S - 3600 });
  // Some watched / in progress.
  [3, 14, 40, 77, 120].forEach((i, k) => {
    const m = movies[i];
    if (m) m.progress = [0.32, 0.61, 0.12, 0.84, 0.45][k]!;
  });
  [5, 22, 61].forEach((i) => {
    const m = movies[i];
    if (m) m.played = true;
  });

  // ---- Series
  const series: VodSeries[] = [];
  let sid = 20001;
  let eid = 300001;
  const addSeries = (catIdx: number, o: Partial<VodSeries> = {}): VodSeries => {
    const seed = SERIES_CATS[catIdx]!;
    const cat = seriesCats[catIdx]!;
    const lang = seed[2];
    const ramadan = /Ramadan/.test(seed[0]);
    const title = o.title ?? titleFor(lang);
    const s = hash(title) % 100000;
    const hasMeta = r() > 0.3;
    const nSeasons = ramadan ? 1 : 1 + Math.floor(Math.pow(r(), 2) * 5);
    const seasons: VodSeason[] = [];
    const stills = r() > 0.35;
    for (let sn = 1; sn <= nSeasons; sn++) {
      const nEp = ramadan ? 30 : lang === "ar" ? 15 + Math.floor(r() * 16) : 6 + Math.floor(r() * 8);
      const eps: VodEpisode[] = [];
      for (let e = 1; e <= nEp; e++) {
        const generic = lang === "ar" || r() > 0.55;
        eps.push({
          id: eid++, episode_no: e,
          title: generic ? (lang === "ar" ? `الحلقة ${e}` : `Episode ${e}`) : `${pick(EN_ADJ)} ${pick(EN_NOUN)}`,
          overview: hasMeta && r() > 0.3 ? (lang === "ar" ? pick(AR_OVERVIEW) : pick(EN_OVERVIEW)) : null,
          still_url: stills ? backdropArt(s + sn * 100 + e) : null,
          runtime_s: (lang === "ar" ? 38 + Math.floor(r() * 12) : 42 + Math.floor(r() * 20)) * 60,
          progress: null, played: false,
        });
      }
      seasons.push({ season_no: sn, name: lang === "ar" ? `الموسم ${sn}` : `Season ${sn}`, poster_url: null, episodes: eps });
    }
    const sr: VodSeries = {
      id: sid++, kind: "series", title, year: 1995 + Math.floor(Math.pow(r(), 0.4) * 32),
      poster_url: r() > 0.06 ? posterArt(title, s) : null,
      rating: hasMeta && r() > 0.2 ? Math.round((5 + r() * 4) * 10) / 10 : null,
      genres: [seed[3] ?? pick(GENRES)],
      tmdb_id: hasMeta ? 950000 + sid : null,
      category_id: cat.id,
      progress: null, played: false,
      overview: hasMeta ? (lang === "ar" ? pick(AR_OVERVIEW) : pick(EN_OVERVIEW)) : null,
      backdrop_url: r() > 0.2 ? backdropArt(s) : null,
      cast: hasMeta ? castFor(s, 8) : [],
      trailer_youtube_id: null,
      seasons,
      ...o,
    };
    if (sr.year && sr.year > 2026) sr.year = 2026;
    cat.count++;
    series.push(sr);
    return sr;
  };
  const totalS = SERIES_CATS.reduce((a, c) => a + c[1], 0);
  SERIES_CATS.forEach((c, i) => {
    const n = Math.max(3, Math.round((c[1] / totalS) * 520));
    for (let k = 0; k < n; k++) addSeries(i);
  });
  libShows.forEach((rec, i) => {
    addSeries([15, 16][i]!, { title: rec.summary.title, year: rec.summary.year, tmdb_id: rec.tmdb_id, library_item_id: rec.summary.id, poster_url: rec.summary.poster_url, backdrop_url: rec.summary.backdrop_url, overview: rec.overview, cast: rec.cast });
  });
  DISCOVER_SHOWS.forEach(([t, tmdb, year], i) => {
    addSeries([15, 16, 20][i]!, { title: t, tmdb_id: tmdb, year, poster_url: posterArt(t, hash(t) % 100000) });
  });
  // In progress: second season, a few episodes in.
  [0, 7, 42].forEach((i, k) => {
    const s = series[i];
    if (!s) return;
    const season = s.seasons[Math.min(s.seasons.length - 1, k === 0 ? 1 : 0)]!;
    const upto = Math.min(season.episodes.length - 1, 3 + k * 2);
    season.episodes.forEach((e, n) => {
      if (n < upto) e.played = true;
      if (n === upto) e.progress = 0.4;
    });
    // Earlier seasons fully watched.
    s.seasons.forEach((ss) => ss.season_no < season.season_no && ss.episodes.forEach((e) => (e.played = true)));
    s.progress = 0.4;
  });

  const movieById = new Map(movies.map((m) => [String(m.id), m]));
  const seriesById = new Map(series.map((s) => [String(s.id), s]));
  const episodeById = new Map<string, { series: VodSeries; season: VodSeason; ep: VodEpisode }>();
  series.forEach((s) => s.seasons.forEach((season) => season.episodes.forEach((ep) => episodeById.set(String(ep.id), { series: s, season, ep }))));
  return { movieCats, seriesCats, movies, series, movieById, seriesById, episodeById };
}

// ---------------------------------------------------------------------------

const card = (x: VodCard): VodCard => ({
  id: x.id, kind: x.kind, title: x.title, year: x.year, poster_url: x.poster_url, rating: x.rating, genres: x.genres,
  tmdb_id: x.tmdb_id, category_id: x.category_id, library_item_id: x.library_item_id, progress: x.progress, played: x.played,
});
const titleKey = (s: string) => s.replace(/^(the|a|an)\s+/i, "").toLowerCase();

function sortCards<T extends VodCard & { added_at?: number | null }>(list: T[], sort: VodQuery["sort"] = "added"): T[] {
  const out = list.slice();
  const byTitle = (a: T, b: T) => (titleKey(a.title) < titleKey(b.title) ? -1 : 1);
  switch (sort) {
    case "rating": return out.sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0) || byTitle(a, b));
    case "year": return out.sort((a, b) => (b.year ?? 0) - (a.year ?? 0) || byTitle(a, b));
    case "title": return out.sort(byTitle);
    default: return out.sort((a, b) => (b.added_at ?? 0) - (a.added_at ?? 0) || Number(b.id) - Number(a.id));
  }
}

let singleton: VodDb | undefined;
export function vodDb(): VodDb {
  return singleton ?? (singleton = build());
}

/** Unified search hook for the main mock (`/search/all` → `vod`). */
export function searchVod(q: string, limit = 30): VodCard[] {
  const n = fold(q.trim());
  if (!n) return [];
  const db = vodDb();
  const hits = (db.movies as VodCard[]).concat(db.series).filter((x) => fold(x.title).indexOf(n) >= 0);
  hits.sort((a, b) => Number(fold(b.title).indexOf(n) === 0) - Number(fold(a.title).indexOf(n) === 0));
  return hits.slice(0, limit).map(card);
}

/** Discover cards: the IPTV twin of a TMDB title, if the provider carries it. */
export function vodIdsFor(tmdbId: number): { vod_movie_id?: VodId; vod_series_id?: VodId } {
  const db = vodDb();
  const m = db.movies.find((x) => x.tmdb_id === tmdbId && !x.library_item_id);
  if (m) return { vod_movie_id: m.id };
  const s = db.series.find((x) => x.tmdb_id === tmdbId && !x.library_item_id);
  return s ? { vod_series_id: s.id } : {};
}

/** Another household member is on the provider's single connection until an admin takes over. */
let holder: VodHolder | null = { user_id: 2, username: "maya", session_id: "live-mock-1", kind: "live", title: "MBC Drama HD", device: "Living room TV" };
const sessions = new Set<string>();

export function mockVod(): VodApi {
  const db = vodDb();
  const pool = (t: VodKind): (VodCard & { added_at?: number | null })[] => (t === "movie" ? db.movies : db.series);

  return {
    async categories(type) {
      await latency();
      return (type === "movie" ? db.movieCats : db.seriesCats).map((c) => ({ ...c }));
    },
    async genres(type) {
      return [...new Set(pool(type).flatMap((item) => item.genres ?? []))].sort();
    },
    async list(type, q) {
      await latency();
      const n = q.q ? fold(q.q.trim()) : "";
      let all = pool(type).filter(
        (x) =>
          (q.category == null || String(x.category_id) === String(q.category)) &&
          (!n || fold(x.title).indexOf(n) >= 0) &&
          (!q.genre || (x.genres ?? []).indexOf(q.genre) >= 0),
      );
      all = sortCards(all, q.sort);
      const start = q.cursor ? parseInt(atob(q.cursor), 10) || 0 : 0;
      const limit = q.limit ?? 60;
      return { items: all.slice(start, start + limit).map(card), next_cursor: start + limit < all.length ? btoa(String(start + limit)) : null };
    },
    async movie(id) {
      await latency();
      const m = db.movieById.get(String(id));
      if (!m) throw new ApiError(404, "Not on your IPTV provider anymore");
      return { ...m, cast: m.cast.slice() };
    },
    async series(id) {
      await latency();
      const s = db.seriesById.get(String(id));
      if (!s) throw new ApiError(404, "Not on your IPTV provider anymore");
      return JSON.parse(JSON.stringify(s)) as VodSeries;
    },
    async lanes(type) {
      await latency();
      const all = pool(type);
      const noun = type === "movie" ? "films" : "series";
      const out: VodLane[] = [];
      const cont = type === "movie"
        ? all.filter((x) => x.progress && !x.played)
        : db.series.filter((s) => s.seasons.some((ss) => ss.episodes.some((e) => e.progress)));
      if (cont.length) out.push({ key: "continue", kind: "continue", title: "Continue watching", items: cont.map(card), see_all: null });
      out.push({ key: "new", kind: "new", title: "Recently added", subtitle: "Fresh from your provider", items: sortCards(all, "added").slice(0, 24).map(card), see_all: { query: "sort=added" } });
      out.push({ key: "top", kind: "top_rated", title: "Top rated", items: sortCards(all.filter((x) => x.rating), "rating").slice(0, 24).map(card), see_all: { query: "sort=rating" } });
      const cats = type === "movie" ? db.movieCats : db.seriesCats;
      const featured = type === "movie" ? [0, 17, 35, 1, 55, 40, 70, 9] : [0, 2, 11, 15, 14, 3];
      featured.forEach((i) => {
        const c = cats[i];
        if (!c) return;
        const items = sortCards(all.filter((x) => x.category_id === c.id), "added").slice(0, 24);
        if (items.length >= 3) out.push({ key: `cat:${c.id}`, kind: "category", title: c.name, subtitle: `${c.count} ${noun}`, items: items.map(card), see_all: { query: `category=${c.id}` } });
      });
      return out;
    },
    async play(body) {
      await sleep(380 + Math.random() * 300);
      let title = "";
      if (body.kind === "movie") {
        const m = db.movieById.get(String(body.id));
        if (!m) throw new ApiError(404, "Not on your IPTV provider anymore");
        title = m.title;
        if (m.title === "Signal Lost") {
          // What a non-admin sees: no names, just "in use".
          throw new ApiError(429, "All 1 connection(s) of this provider are in use by other viewers right now. Try again in a moment.", 120);
        }
      } else {
        const e = db.episodeById.get(String(body.id));
        if (!e) throw new ApiError(404, "This episode isn't on your IPTV provider anymore");
        title = `${e.series.title} · S${e.season.season_no} E${e.ep.episode_no}`;
      }
      if (holder && body.takeover) holder = null;
      if (holder) {
        const h = holder;
        throw new ApiError(409, "All 1 connection(s) of this provider are in use by other users. Retry with takeover=true to stop their stream.", undefined, {
          error: "in use",
          takeover: true,
          holders: [{ user_id: h.user_id, username: h.username, session_id: h.session_id, channel_id: 13, channel_name: h.title, since: NOW_S - 1500 }],
        });
      }
      const sid = `vod-${Math.random().toString(36).slice(2, 10)}`;
      sessions.add(sid);
      const arabic = /[\u0600-\u06ff]/.test(title);
      const res: DecideResponse = {
        session_id: sid,
        mode: "remux",
        url: SAMPLE_VIDEO,
        audio: [
          { index: 1, codec: "aac", channels: 2, language: arabic ? "ara" : "eng", title: null, default: true },
          ...(arabic ? [] : [{ index: 2, codec: "ac3", channels: 6, language: "fre", title: "Français", default: false }]),
        ],
        subtitles: arabic ? [] : [{ index: 3, stream_id: 1, language: "ara", title: null, forced: false, external: false, url: "/mock-media/subs.vtt?lang=ara" }],
        decision: {
          mode: "remux",
          reasons: ["IPTV source: container repackaged for this device"],
          audio_index: body.audio_index ?? 1,
          subtitle_index: body.subtitle_index ?? null,
          video: { type: "copy" },
          audio: { type: "copy" },
          subtitle: body.subtitle_index === 3 ? { type: "sidecar", format: "vtt" } : { type: "none" },
        },
      };
      // Provider files are typically 1080p H.264 at ~4.5 Mbps.
      const qd = mockQuality({ width: 1920, height: 1080, hdr: "none", kbps: 4500 }, body, res.decision);
      return Object.assign(res, { mode: qd.mode, decision: qd.decision, quality: qd.quality, bitrate_kbps: qd.bitrate_kbps, quality_options: qd.quality_options });
    },
    async stopHolder(h) {
      await sleep(300);
      if (holder && h.session_id === holder.session_id) holder = null;
    },
  };
}

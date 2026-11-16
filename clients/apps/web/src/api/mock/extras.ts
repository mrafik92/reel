import type { DiscoverCard, DownloadItem, PersonCredit, PersonDetail, Rating, Review, Reviews, MediaType } from "../types";
import type { Db } from "./fixtures";
import { castFor, personId } from "./fixtures";
import type { TmdbEntry } from "./discover";
import { hash, headshotArt, rng } from "./art";

const BIO_BITS = [
  "began acting in regional theatre before a breakout role in an independent drama that travelled the festival circuit.",
  "is known for quiet, precise performances and a habit of disappearing into character for months at a time.",
  "trained as a dancer, which shows in the physical comedy that later became a signature.",
  "has worked with many of the same directors repeatedly, forming a loose repertory company across a decade of films.",
  "splits time between screen work and the stage, returning every few years to a long-running company in Lisbon.",
  "made a directorial debut in 2023 with a small, much-praised documentary about a family-run cinema.",
];
const PLACES = ["Lyon, France", "Accra, Ghana", "Gothenburg, Sweden", "Valparaíso, Chile", "Osaka, Japan", "Cork, Ireland", "Montréal, Canada"];

/** Person detail: library films they appear in + some TMDB catalogue titles. */
export function mockPerson(db: Db, tmdb: TmdbEntry[], id: number, cardOf: (c: TmdbEntry) => DiscoverCard): PersonDetail | undefined {
  let name: string | undefined;
  let photo: string | null | undefined;
  const credits: PersonCredit[] = [];
  db.byId.forEach((r) => {
    if (r.summary.kind !== "movie" && r.summary.kind !== "show") return;
    const c = r.cast.find((x) => x.person_tmdb_id === id);
    if (!c) return;
    name = c.name;
    photo = c.photo_url;
    credits.push({
      tmdb_id: r.tmdb_id, media_type: r.summary.kind === "show" ? "tv" : "movie", title: r.summary.title, year: r.summary.year,
      poster_url: r.summary.poster_url, backdrop_url: r.summary.backdrop_url, overview: r.overview, vote_average: r.rating ?? null,
      status: "available", progress: null, role: "cast", character: c.character ?? null, library_item_id: r.summary.id,
    });
  });
  // Discover titles cast via castFor(seed) too.
  tmdb.forEach((t) => {
    const c = castOfTmdb(t).find((x) => x.person_tmdb_id === id);
    if (!c) return;
    name = name ?? c.name;
    photo = photo ?? c.photo_url;
    if (!credits.some((x) => x.tmdb_id === t.tmdb_id)) credits.push({ ...cardOf(t), role: "cast", character: c.character ?? null });
  });
  if (!name) return undefined;
  const r = rng(id);
  // Some people also direct or write.
  const pool = tmdb.filter((t) => t.status === "none");
  for (let i = 0; i < (r() > 0.6 ? 2 : 0); i++) {
    const t = pool[Math.floor(r() * pool.length)];
    if (t && !credits.some((x) => x.tmdb_id === t.tmdb_id)) credits.push({ ...cardOf(t), role: "crew", job: i === 0 ? "Director" : "Writer" });
  }
  // Pad with a few more catalogue titles so the grid has something to Add.
  for (let i = 0; i < 5; i++) {
    const t = pool[Math.floor(r() * pool.length)];
    if (t && !credits.some((x) => x.tmdb_id === t.tmdb_id)) {
      credits.push({ ...cardOf(t), role: "cast", character: castOfTmdb(t)[0]?.character ?? null });
    }
  }
  // TMDB vote counts / popularity for the sort control (#141), stable per title.
  credits.forEach((c) => {
    const v = rng(c.tmdb_id + 7);
    c.vote_count = Math.floor(30 + v() * v() * 12_000);
    c.popularity = Math.round((0.5 + v() * v() * 90) * 10) / 10;
  });
  // Edge cases: a thinly-voted 9.6 (must not top "Highest rated") and an unrated title.
  const thin = credits[credits.length - 1];
  if (thin) Object.assign(thin, { vote_average: 9.6, vote_count: 4, popularity: 0.8 });
  const unrated = credits.length > 3 ? credits[credits.length - 2] : undefined;
  if (unrated) Object.assign(unrated, { vote_average: null, vote_count: 0, popularity: 0.3 });
  credits.sort((a, b) => (b.year ?? 0) - (a.year ?? 0));
  const born = 1950 + Math.floor(r() * 45);
  const first = name.split(" ")[0];
  return {
    tmdb_id: id,
    name,
    photo_url: photo ?? headshotArt(name),
    known_for_department: r() > 0.85 ? "Directing" : "Acting",
    birthday: `${born}-0${1 + Math.floor(r() * 9)}-1${Math.floor(r() * 9)}`,
    deathday: r() > 0.93 ? `${born + 60 + Math.floor(r() * 10)}-03-02` : null,
    place_of_birth: PLACES[Math.floor(r() * PLACES.length)]!,
    imdb_id: `nm${String(id).padStart(7, "0")}`,
    biography: `${name} ${BIO_BITS[id % BIO_BITS.length]} ${first} ${BIO_BITS[(id + 2) % BIO_BITS.length]}\n\nAfter a run of supporting parts in television, ${first} ${BIO_BITS[(id + 4) % BIO_BITS.length]} Critics have singled out ${first}'s work for its restraint, and the last few years have brought leading roles, a string of festival awards and a growing interest in producing.`,
    credits,
  };
}

export function castOfTmdb(t: { tmdb_id: number }) {
  return castFor(t.tmdb_id % 100_000, 8);
}

const REVIEWERS = ["filmfanatic88", "Ines R.", "the_projectionist", "Chris Lambert", "MidnightMatinee", "Dev K."];
const REVIEW_TEXT = [
  "A rare film that trusts its audience. The pacing is deliberate — some will call it slow — but every scene earns its place, and the final twenty minutes land with real weight. The score does a lot of quiet work too.",
  "Gorgeous to look at and the lead performance is extraordinary, but the script loses its nerve in the third act and settles for a tidier ending than the story deserves. Still well worth watching on the biggest screen you can find.",
  "I went in knowing nothing and came out wanting to tell everyone. **Funny, sad and strange** in equal measure. The supporting cast is uniformly great, and the production design makes the setting feel like a character in its own right.\n\nWatch it with the sound up.",
  "Competent but forgettable. Nothing here is bad, exactly, but nothing surprised me either. If you love the genre you'll have a decent evening; if not, there are better entry points.",
];

/** Ratings/reviews: richer for some titles, TMDB-only for others (no OMDb key). */
export function mockReviews(media: MediaType, id: number, vote: number | null | undefined, imdbId?: string | null): Reviews {
  const r = rng(id * 7 + 1);
  const tmdbScore = vote ?? Math.round((5.8 + r() * 3) * 10) / 10;
  const ratings: Rating[] = [{ source: "tmdb", label: "TMDB", value: `${Math.round(tmdbScore * 10)}%`, score_100: Math.round(tmdbScore * 10), votes: 200 + Math.floor(r() * 9000) }];
  const omdb = id % 5 !== 0; // every fifth title: TMDB only
  if (omdb) {
    const imdb = Math.min(9.6, Math.round((tmdbScore + (r() - 0.5)) * 10) / 10);
    ratings.unshift({ source: "imdb", label: "IMDb", value: `${imdb.toFixed(1)}/10`, score_100: Math.round(imdb * 10), votes: 4000 + Math.floor(r() * 400000), url: `https://www.imdb.com/title/${imdbId ?? "tt" + id}/` });
    const rt = Math.max(12, Math.min(100, Math.round(imdb * 10 + (r() - 0.4) * 30)));
    ratings.splice(1, 0, { source: "rotten_tomatoes", label: "Rotten Tomatoes", value: `${rt}%`, score_100: rt });
    if (media === "movie") {
      const mc = Math.max(20, Math.min(98, Math.round(rt * 0.8 + r() * 15)));
      ratings.splice(2, 0, { source: "metacritic", label: "Metacritic", value: `${mc}/100`, score_100: mc });
    }
  }
  const n = id % 7 === 3 ? 0 : 1 + Math.floor(r() * 4);
  const reviews: Review[] = [];
  for (let i = 0; i < n; i++) {
    const author = REVIEWERS[(id + i) % REVIEWERS.length]!;
    reviews.push({
      source: i % 3 === 2 ? "trakt" : "tmdb",
      author,
      avatar_url: i % 2 ? headshotArt(author) : null,
      rating: i === 3 ? null : Math.round((5 + r() * 5) * 10) / 10,
      content: REVIEW_TEXT[(id + i) % REVIEW_TEXT.length]! + (i === 0 ? " " + REVIEW_TEXT[(id + i + 1) % REVIEW_TEXT.length] : ""),
      created_at: new Date(Date.UTC(2024, (id + i) % 12, 1 + (i * 7) % 27)).toISOString(),
      url: `https://www.themoviedb.org/review/${id}${i}`,
    });
  }
  const slug = `title-${id}`;
  const links: Reviews["links"] = [
    { source: "imdb", url: `https://www.imdb.com/title/${imdbId ?? "tt" + id}/` },
    { source: "rotten_tomatoes", url: `https://www.rottentomatoes.com/${media === "tv" ? "tv" : "m"}/${slug}` },
    { source: "tmdb", url: `https://www.themoviedb.org/${media}/${id}` },
  ];
  if (media === "movie") links.splice(2, 0, { source: "letterboxd", url: `https://letterboxd.com/tmdb/${id}/` });
  return { ratings, reviews, links };
}

// ---------------------------------------------------------------------------
// Arr queue simulation (all activity, not only Reel requests).

interface QueueSim {
  item: DownloadItem;
  rate: number;
  /** Seconds left in a non-downloading state. */
  hold: number;
}

export function createQueue(db: Db): { tick(): void; list(): DownloadItem[] } {
  const shows = db.byLibrary.get(2)!.map((i) => db.byId.get(i)!);
  const films = db.byLibrary.get(1)!.map((i) => db.byId.get(i)!);
  const harbour = shows[0]!;
  const film = films[8]!;
  let nid = 9000;
  const mk = (p: Partial<DownloadItem> & Pick<DownloadItem, "title" | "media_type" | "state">, rate: number, hold = 0): QueueSim => ({
    item: { id: `${p.media_type === "tv" ? 2 : 1}:${nid++}`, arr_server_id: p.media_type === "tv" ? 2 : 1, arr_name: p.media_type === "tv" ? "Sonarr" : "Radarr", progress: 0, ...p } as DownloadItem,
    rate,
    hold,
  });
  const sims: QueueSim[] = [
    mk({ media_type: "tv", title: harbour.summary.title, tmdb_id: harbour.tmdb_id, library_item_id: harbour.summary.id, poster_url: harbour.summary.poster_url,
      season: 3, episode: 9, episode_title: "The Night Ferry", state: "downloading", progress: 0.42, size_bytes: 2.1e9,
      release_title: "Harbour.Lights.S03E09.1080p.WEB.H264-NTb" }, 0.012),
    mk({ media_type: "tv", title: harbour.summary.title, tmdb_id: harbour.tmdb_id, library_item_id: harbour.summary.id, poster_url: harbour.summary.poster_url,
      season: 3, episode: 10, episode_title: "Breakwater", state: "queued", size_bytes: 2.3e9, release_title: "Harbour.Lights.S03E10.1080p.WEB.H264-NTb" }, 0.015, 8),
    mk({ media_type: "movie", title: film.summary.title, tmdb_id: film.tmdb_id, library_item_id: film.summary.id, poster_url: film.summary.poster_url,
      state: "downloading", progress: 0.71, size_bytes: 18.4e9, release_title: `${film.summary.title.replace(/\s+/g, ".")}.${film.summary.year}.2160p.UHD.BluRay.REMUX.HDR.HEVC.TrueHD.7.1-FGT` }, 0.004),
    mk({ media_type: "movie", title: "The Weather Thief", tmdb_id: 700023, state: "stalled", progress: 0.18, size_bytes: 6.2e9,
      release_title: "The.Weather.Thief.2025.1080p.BluRay.x264-SPARKS", error: "No connections to peers for 14 minutes" }, 0),
    mk({ media_type: "movie", title: "Low Tide Hotel", tmdb_id: 700024, state: "failed", progress: 0.93, size_bytes: 4.4e9,
      release_title: "Low.Tide.Hotel.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", error: "Import failed: no video files found in download folder" }, 0),
  ];
  return {
    tick() {
      for (const s of sims) {
        const it = s.item;
        if (s.hold > 0) {
          s.hold -= 1;
          if (s.hold <= 0 && it.state === "queued") it.state = "downloading";
          if (s.hold <= 0 && it.state === "importing") {
            // Done: start over so the demo keeps moving.
            it.state = "queued";
            it.progress = 0;
            s.hold = 6;
          }
          continue;
        }
        if (it.state !== "downloading") continue;
        it.progress = Math.min(1, it.progress + s.rate * (0.6 + Math.random() * 0.8));
        if (it.progress >= 1) {
          it.state = "importing";
          s.hold = 5;
        }
      }
    },
    list() {
      return sims.map((s) => {
        const it = s.item;
        const speed = it.state === "downloading" ? Math.round((it.size_bytes ?? 2e9) * s.rate) : null;
        const left = it.size_bytes ? Math.round(it.size_bytes * (1 - it.progress)) : null;
        return { ...it, speed_bps: speed, sizeleft_bytes: left, eta_seconds: speed && left ? Math.round(left / speed) : null };
      });
    },
  };
}

export { hash, personId };

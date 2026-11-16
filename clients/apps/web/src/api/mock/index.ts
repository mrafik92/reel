import type { Api } from "../index";
import { ApiError } from "../http";
import type { ItemSummary } from "@reel/ui";
import type { ReelEvent } from "@reel/sdk";
import type {
  AdminSession, ArrServer, DecideResponse, DiscoverCard, DownloadClient, InviteDto, Me, NotificationTarget,
  ProgressDto, RequestDto, RequestState, UserDto,
} from "../types";
import { buildDb, detail, home, importTitle, tmdbIndex, SAMPLE_VIDEO } from "./fixtures";
import { buildTmdb, type TmdbEntry } from "./discover";
import { castOfTmdb, createQueue, mockPerson, mockReviews } from "./extras";
import type { DownloadItem } from "../types";
import { enrichSummaries, genreCounts, lanes, libraryRecs, matches, searchAll, sortRecs } from "./browse";
import { searchVod, vodIdsFor } from "./vod";
import { mockSelected } from "./select";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const latency = () => sleep(120 + Math.random() * 220);
const nowS = () => Math.floor(Date.now() / 1000);

function fold(s: string): string {
  return s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");
}

type Limits = Me["limits"];
const lim = (p: Partial<Limits> = {}): Limits => ({
  auto_approve: false, max_bitrate_kbps: null, max_streams: null, max_transcodes: null,
  quota_window_days: 7, request_quota_movies: null, request_quota_tv: null, ...p,
});

const ADMIN: Me = {
  id: 1, username: "rafik", role: "admin",
  limits: lim({ auto_approve: true }),
  direct_live: true,
};

interface Sim {
  req: RequestDto;
  progress: number;
  t: number;
  /** Mock-only: make this one stall partway, or fail at the end. */
  script?: "stall" | "fail";
  release?: string;
}

export interface MockApi extends Api {
  events: { on(type: string, h: (e: ReelEvent) => void): () => void };
}

/** In-browser stand-in for reel-server (`?mock=1`): fixtures + a simulated download pipeline. */
export function createMockApi(): MockApi {
  const db = buildDb();
  enrichSummaries(db);
  if (typeof console !== "undefined") console.info("[reel] mock mode — fixture data, no server calls");
  const need = <T>(v: T | undefined, what: string): T => {
    if (v === undefined) throw new ApiError(404, `${what} not found`);
    return v;
  };

  // ---- Discover catalogue ----------------------------------------------------
  const libFilms = db.byLibrary.get(1)!.slice(0, 5).map((id) => db.byId.get(id)!);
  const libShows = db.byLibrary.get(2)!.slice(0, 2).map((id) => db.byId.get(id)!);
  const tmdb: TmdbEntry[] = buildTmdb(
    libFilms.map((r) => ({ tmdb_id: r.tmdb_id, title: r.summary.title, year: r.summary.year, media: "movie" as "movie" | "tv", poster: r.summary.poster_url }))
      .concat(libShows.map((r) => ({ tmdb_id: r.tmdb_id, title: r.summary.title, year: r.summary.year, media: "tv" as const, poster: r.summary.poster_url }))),
  );

  // ---- Requests + simulated download tracker --------------------------------
  let nextReq = 40;
  const requests: RequestDto[] = [];
  const sims = new Map<number, Sim>();
  const handlers = new Set<(e: ReelEvent) => void>();
  const emit = (s: Sim) => {
    const speed = s.req.status === "downloading" ? Math.round(9e6 + Math.sin(s.t / 3) * 3e6) : null;
    const ev: ReelEvent = {
      type: "request_progress",
      request_id: String(s.req.id),
      tmdb_id: s.req.tmdb_id,
      state: s.req.status,
      progress: s.progress,
      eta_seconds: speed ? Math.round(((1 - s.progress) * 3.8e9) / speed) : null,
      speed_bps: speed,
    };
    handlers.forEach((h) => h(ev));
  };
  const setStatus = (s: Sim, st: RequestState) => {
    s.req.status = st;
    s.req.updated_at = nowS();
    s.t = 0;
    const card = tmdb.find((c) => c.tmdb_id === s.req.tmdb_id);
    if (card) card.status = st === "available" ? "available" : st === "declined" ? "none" : "requested";
  };
  const step = (s: Sim) => {
    s.t++;
    switch (s.req.status) {
      case "approved": if (s.t > 2) setStatus(s, "searching"); break;
      case "searching": if (s.t > 5) setStatus(s, "queued"); break;
      case "queued": if (s.t > 3) setStatus(s, "downloading"); break;
      case "downloading":
        s.progress = Math.min(1, s.progress + 0.025 + Math.random() * 0.02);
        if (s.script === "stall" && s.progress > 0.5) {
          s.script = undefined;
          setStatus(s, "stalled");
        } else if (s.script === "fail" && s.progress > 0.7) {
          s.script = undefined;
          setStatus(s, "failed");
        } else if (s.progress >= 1) setStatus(s, "importing");
        break;
      case "stalled": if (s.t > 12) setStatus(s, "downloading"); break;
      case "importing":
        if (s.t > 5) {
          const c = tmdb.find((x) => x.tmdb_id === s.req.tmdb_id);
          importTitle(db, { title: s.req.title ?? "Untitled", year: c?.year, tmdb_id: s.req.tmdb_id, media_type: s.req.media_type, overview: c?.overview, genres: c?.genres });
          setStatus(s, "available");
          sims.delete(s.req.id);
        }
        break;
      default:
        return false;
    }
    return true;
  };
  setInterval(() => {
    sims.forEach((s) => {
      if (step(s) || s.req.status === "available") emit(s);
    });
  }, 500);

  // Radarr/Sonarr queue: Reel's own requests plus unrelated arr activity.
  const queue = createQueue(db);
  const downloadList = (): DownloadItem[] => {
    const fromRequests: DownloadItem[] = [];
    sims.forEach((s) => {
      const st = s.req.status;
      if (st !== "queued" && st !== "downloading" && st !== "stalled" && st !== "importing" && st !== "failed") return;
      const size = 3.8e9;
      const speed = st === "downloading" ? Math.round(9e6 + Math.sin(s.t / 3) * 3e6) : null;
      fromRequests.push({
        id: `${s.req.media_type === "tv" ? 2 : 1}:${5000 + s.req.id}`, arr_server_id: s.req.media_type === "tv" ? 2 : 1, arr_name: s.req.media_type === "tv" ? "Sonarr" : "Radarr",
        media_type: s.req.media_type, title: s.req.title ?? "", tmdb_id: s.req.tmdb_id, state: st, progress: s.progress,
        size_bytes: size, sizeleft_bytes: Math.round(size * (1 - s.progress)), speed_bps: speed,
        eta_seconds: speed ? Math.round((size * (1 - s.progress)) / speed) : null, request_id: s.req.id, poster_url: s.req.poster_url,
        release_title: `${(s.req.title ?? "").replace(/\s+/g, ".")}.2160p.WEB-DL.DDP5.1.HEVC-GRP`,
        season: s.req.media_type === "tv" ? 1 : null, episode: s.req.media_type === "tv" ? 1 : null,
      });
    });
    return fromRequests.concat(queue.list());
  };
  setInterval(() => queue.tick(), 1000);
  setInterval(() => {
    const items = downloadList();
    if (items.length) handlers.forEach((h) => h({ type: "downloads", items } as unknown as ReelEvent));
  }, 2000);

  const addRequest = (card: TmdbEntry, user: { id: number; name: string }, status: RequestState, extra: Partial<Sim> = {}) => {
    const req: RequestDto = {
      id: nextReq++, user_id: user.id, media_type: card.media_type, tmdb_id: card.tmdb_id, title: card.title,
      poster_url: card.poster_url, status, created_at: nowS() - Math.floor(Math.random() * 86400 * 3), updated_at: nowS(),
      seasons: card.media_type === "tv" ? card.seasons : [], approved_by: status === "requested" ? null : 1,
      arr_kind: card.media_type === "tv" ? "sonarr" : "radarr",
    };
    requests.unshift(req);
    (req as RequestDto & { username?: string }).username = user.name;
    card.status = status === "available" ? "available" : "requested";
    const s: Sim = { req, progress: extra.progress ?? 0, t: 0, ...extra };
    if (status !== "requested" && status !== "declined" && status !== "available") sims.set(req.id, s);
    return s;
  };
  // Seed: one downloading, one stalled-prone, one failed, one waiting for approval.
  const pick = (title: string) => tmdb.find((c) => c.title === title)!;
  addRequest(pick("Halflight"), { id: 1, name: "rafik" }, "downloading", { progress: 0.34 });
  addRequest(pick("Static Bloom"), { id: 1, name: "rafik" }, "downloading", { progress: 0.12, script: "stall" });
  addRequest(pick("Granite"), { id: 2, name: "maya" }, "failed", { progress: 0.41 });
  addRequest(pick("Tidewater"), { id: 3, name: "theo" }, "requested");
  addRequest(pick("Paperweight"), { id: 2, name: "maya" }, "available", { progress: 1 });

  const progressDto = (s: Sim): ProgressDto => ({
    request_id: s.req.id, tmdb_id: s.req.tmdb_id, media_type: s.req.media_type, state: s.req.status, progress: s.progress,
    items: [], release_title: s.release ?? `${(s.req.title ?? "").replace(/\s+/g, ".")}.2160p.WEB-DL.DDP5.1.HEVC`,
    size_bytes: 3.8e9, eta_seconds: null, speed_bps: null,
  });
  const cardOf = (c: TmdbEntry): DiscoverCard => {
    const sim = Array.from(sims.values()).find((s) => s.req.tmdb_id === c.tmdb_id);
    const { genres: _g, runtime_minutes: _r, seasons: _s, lists: _l, ...card } = c;
    const lib = tmdbIndex(db).get(c.tmdb_id);
    // The IPTV provider's twin of this title (Play now (IPTV) next to Add).
    return { ...card, progress: sim ? progressDto(sim) : null, ...(lib ? { library_item_id: lib } : {}), ...vodIdsFor(c.tmdb_id) };
  };

  // ---- Admin fixtures ----------------------------------------------------------
  const users: UserDto[] = [
    { id: 1, username: "rafik", role: "admin", disabled: false, library_ids: [], created_at: nowS() - 86400 * 90, limits: lim({ auto_approve: true, quota_window_days: 7 }) },
    { id: 2, username: "maya", role: "user", disabled: false, library_ids: [1, 2], created_at: nowS() - 86400 * 30, limits: lim({ auto_approve: true, max_streams: 2, max_transcodes: 1, max_bitrate_kbps: 20000, request_quota_movies: 10, request_quota_tv: 3, quota_window_days: 7 }) },
    { id: 3, username: "theo", role: "user", disabled: false, library_ids: [3], created_at: nowS() - 86400 * 4, limits: lim({ auto_approve: false, max_streams: 1, max_transcodes: 1, quota_window_days: 7, request_quota_movies: 3 }) },
    { id: 4, username: "guest", role: "user", disabled: true, library_ids: [1], created_at: nowS() - 86400 * 120, limits: lim({ auto_approve: false, quota_window_days: 7 }) },
  ];
  const invites: InviteDto[] = [
    { id: 1, code: "Q7WK-PLMN", status: "pending", url: `${location.origin}/invite/Q7WK-PLMN`, library_ids: [1, 2], limits: lim({ max_streams: 2, quota_window_days: 7 }), expires_at: nowS() + 86400 * 5 },
    { id: 2, code: "ZX81-AABB", status: "used", url: `${location.origin}/invite/ZX81-AABB`, library_ids: [3], limits: lim({ quota_window_days: 7 }), used_by: 3, used_at: nowS() - 86400 * 4 },
  ];
  const arr: ArrServer[] = [
    { id: 1, kind: "radarr", name: "Radarr", url: "http://radarr:7878", api_key_hint: "••••3f9a", is_4k: false, default_profile: 4, default_root: "/media/films" },
  ];
  const dlc: DownloadClient[] = [{ id: 1, kind: "qbittorrent", url: "http://qbittorrent:8080", username: "admin", password_set: true }];
  const notes: NotificationTarget[] = [{ id: 1, kind: "discord", config: { webhook_url: "https://discord.com/api/webhooks/••••" } as never, events: ["available", "failed"] }];
  const playback = new Map<string, AdminSession>();
  // Dev hook: end every session the way the server's idle reaper does (#95), e.g. from Playwright.
  (globalThis as { reelMockReap?: () => number }).reelMockReap = () => {
    const n = playback.size;
    playback.clear();
    return n;
  };
  playback.set("s-demo", {
    id: "s-demo", user_id: 2, username: "maya", item_id: 104, item_title: "Small Hours", file_id: 1041, active: true,
    mode: "transcode", hwaccel: "vaapi", encoder: "hevc_vaapi (low power)", position_s: 1880, progress: 0.32, bitrate_kbps: 8000, started_at: nowS() - 1900,
    last_ping: nowS() - 4, device_id: "lg-oled",
    reasons: ["Audio codec truehd not supported by webos", "Bitrate 62 Mbps above user cap 20 Mbps"],
    decision: { mode: "transcode", reasons: [], video: { type: "encode", codec: "hevc", width: 1920, height: 1080, bitrate_kbps: 8000, tonemap: false }, audio: { type: "encode", codec: "eac3", channels: 6, bitrate_kbps: 640 }, subtitle: { type: "none" } },
  });

  const id = (n: string | number) => Number(n);
  const ok = async () => {
    await latency();
  };

  const api: MockApi = {
    mock: true,
    events: {
      on(_type, h) {
        handlers.add(h);
        return () => handlers.delete(h);
      },
    },

    // Auth: always signed in as an admin in mock mode.
    setupStatus: async () => ({ needs_setup: false }),
    setup: async () => ({ token: "mock", expires_at: nowS() + 86400 * 30, user: ADMIN }),
    login: async (c) => {
      await latency();
      if (c.password.length < 3) throw new ApiError(401, "Wrong username or password");
      return { token: "mock", expires_at: nowS() + 86400 * 30, user: { ...ADMIN, username: c.username } };
    },
    logout: ok,
    me: async () => ADMIN,
    inviteInfo: async (code) => ({ valid: code.length > 3, library_names: ["Films", "Series"], expires_at: nowS() + 86400 * 3 }),
    acceptInvite: async (_code, c) => ({ token: "mock", expires_at: nowS() + 86400 * 30, user: { ...ADMIN, role: "user", username: c.username } }),

    // Library
    async libraries() {
      await latency();
      return db.libraries.map((l) => ({ ...l }));
    },
    async createLibrary(b) {
      await latency();
      const lib = { id: db.libraries.length + 1, item_count: 0, ...b };
      db.libraries.push(lib);
      db.byLibrary.set(lib.id, []);
      return lib;
    },
    scanLibrary: async (lid) => {
      await latency();
      let n = 0;
      const total = db.byLibrary.get(id(lid))?.length || 40;
      const t = setInterval(() => {
        n = Math.min(total, n + Math.ceil(total / 12));
        handlers.forEach((h) => h({ type: "scan_progress", library_id: String(lid), scanned: n, total }));
        if (n >= total) clearInterval(t);
      }, 400);
    },
    async libraryItems(lid, { sort, cursor, limit = 60, seed, filters }) {
      await latency();
      need(db.byLibrary.get(id(lid)), "Library");
      enrichSummaries(db);
      const all = sortRecs(libraryRecs(db, id(lid)).filter((r) => matches(r, filters)), sort, seed).map((r) => r.summary);
      const start = cursor ? parseInt(atob(cursor), 10) || 0 : 0;
      const items = all.slice(start, start + limit).map((s) => ({ ...s }));
      return { items, next_cursor: start + limit < all.length ? btoa(String(start + limit)) : null };
    },
    async genres(lid) {
      await latency();
      return genreCounts(db, lid != null ? id(lid) : undefined);
    },
    async browseLanes(lid) {
      await latency();
      need(db.byLibrary.get(id(lid)), "Library");
      enrichSummaries(db);
      return lanes(db, id(lid));
    },
    async pick(lid, filters) {
      await sleep(260 + Math.random() * 200);
      enrichSummaries(db);
      const pool = libraryRecs(db, id(lid)).filter((r) => matches(r, filters));
      if (!pool.length) throw new ApiError(404, "Nothing matches those filters");
      return { ...pool[Math.floor(Math.random() * pool.length)]!.summary };
    },
    async searchAll(q, limit = 30) {
      await sleep(140 + Math.random() * 160);
      let tm: DiscoverCard[] = [];
      try {
        tm = await api.discoverSearch(q);
      } catch {
        /* TMDB off */
      }
      return { ...searchAll(db, q, limit, tm), vod: searchVod(q, limit) };
    },
    async item(iid) {
      await latency();
      return need(detail(db, id(iid)), "Item");
    },
    async children(iid) {
      await latency();
      const rec = need(db.byId.get(id(iid)), "Item");
      return rec.childIds.map((c) => ({ ...db.byId.get(c)!.summary }));
    },
    async home() {
      await latency();
      return home(db);
    },
    async search(q, limit = 60) {
      await sleep(80 + Math.random() * 120);
      const needle = fold(q.trim());
      if (!needle) return [];
      const out: ItemSummary[] = [];
      db.byId.forEach((r) => {
        if ((r.summary.kind === "movie" || r.summary.kind === "show") && fold(r.summary.title).indexOf(needle) >= 0) out.push(r.summary);
      });
      out.sort((a, b) => Number(fold(b.title).indexOf(needle) === 0) - Number(fold(a.title).indexOf(needle) === 0));
      return out.slice(0, limit);
    },
    async setPlayed(iid, played) {
      await latency();
      const r = need(db.byId.get(id(iid)), "Item");
      const mark = (x: typeof r) => {
        x.summary.played = played;
        x.summary.progress = null;
        x.childIds.forEach((c) => mark(db.byId.get(c)!));
      };
      mark(r);
    },

    // Playback
    async decide(fileId, body) {
      await latency();
      const itemId = Math.floor(id(fileId) / 10);
      const d = need(detail(db, itemId), "File");
      const f = need(d.files.find((x) => x.id === id(fileId)), "File");
      if (body.subtitle_index === 999) throw new ApiError(429, "Transcode limit reached (1 of 1 in use)", 60);
      // Automatic track choice from the (mock) preferences; explicit indexes win.
      const forCast = !!(body as { for_cast?: boolean }).for_cast;
      const selected = mockSelected(f, body.audio_index, body.subtitle_index, forCast);
      const sub = f.subtitles.find((s) => s.index === selected.subtitle_index);
      const imageSub = !!sub && /pgs|dvd/.test(sub.codec);
      const mode: DecideResponse["mode"] = imageSub ? "transcode" : f.container === "mkv" ? "remux" : f.audio.some((a) => a.codec === "truehd") ? "audio_transcode" : "direct_play";
      const reasons: string[] = [];
      if (f.container === "mkv") reasons.push(`Container mkv not in profile (${body.preset ?? body.profile?.name ?? "web"})`);
      if (imageSub) reasons.push("Image subtitles (PGS) must be burned in");
      if (f.video && f.video.hdr !== "none") reasons.push(`${f.video.hdr.toUpperCase()} passthrough not supported by this display`);
      const sid = `mock-${Math.random().toString(36).slice(2, 10)}`;
      const res: DecideResponse = {
        session_id: sid, mode, audio: f.audio,
        // Cast receivers get absolute URLs on the LAN base.
        url: forCast ? new URL(SAMPLE_VIDEO, location.href).href : SAMPLE_VIDEO,
        subtitles: f.subtitles.map((s, i) => ({
          index: s.index, stream_id: itemId * 100 + i, language: s.language, title: s.title, forced: s.forced, external: s.external,
          url: /pgs|dvd/.test(s.codec) ? null : `/mock-media/subs.vtt?lang=${s.language ?? "und"}`,
        })),
        decision: {
          mode, reasons, audio_index: selected.audio_index ?? f.audio[0]?.index ?? null, subtitle_index: selected.subtitle_index,
          video: mode === "transcode" ? { type: "encode", codec: "h264", width: 1920, height: 1080, bitrate_kbps: 8000, tonemap: f.video?.hdr !== "none" } : { type: "copy" },
          audio: mode === "audio_transcode" || mode === "transcode" ? { type: "encode", codec: "aac", channels: 2, bitrate_kbps: 192 } : { type: "copy" },
          subtitle: imageSub ? { type: "burn_in" } : sub ? { type: "sidecar", format: "vtt" } : { type: "none" },
        },
      };
      playback.set(sid, {
        // Multi-episode files: credit the episode the client says it's watching.
        id: sid, user_id: 1, username: "rafik", item_id: body.item_id ?? f.item_ids?.[0] ?? itemId,
        item_title: body.item_id ? detail(db, body.item_id)?.title ?? d.title : d.title, file_id: f.id, active: true, mode,
        hwaccel: mode === "transcode" ? "vaapi" : "none", encoder: mode === "transcode" ? "h264_vaapi (low power)" : undefined, position_s: body.start_s ?? 0, progress: null,
        bitrate_kbps: null, started_at: nowS(), last_ping: nowS(), device_id: body.device_id, reasons, decision: res.decision,
      });
      return Object.assign(res, { selected });
    },
    async progress(sid, b) {
      const s = playback.get(sid);
      // Like the server: a session that was reaped or never existed is a 404.
      if (!s) throw new ApiError(404, "Not found");
      s.position_s = b.position_s;
      s.last_ping = nowS();
      const rec = s.item_id != null ? db.byId.get(s.item_id) : undefined;
      const d = rec?.runtime_s;
      if (rec && d) {
        // The sample clip is short; scale so Continue watching moves visibly.
        const p = Math.min(0.97, Math.max(rec.summary.progress ?? 0, b.position_s / 90));
        rec.summary.progress = p > 0.95 ? null : p;
        if (p > 0.95) rec.summary.played = true;
      }
    },
    async stopPlayback(sid) {
      playback.delete(sid);
    },

    // Discover & requests
    async discover(list, media = "all", page = 1) {
      await latency();
      const all = tmdb.filter((c) => c.lists.indexOf(list) >= 0 && (media === "all" || c.media_type === media));
      const per = 20;
      return { page, total_pages: Math.max(1, Math.ceil(all.length / per)), total_results: all.length, results: all.slice((page - 1) * per, page * per).map(cardOf) };
    },
    async discoverSearch(q, media = "all") {
      await sleep(150 + Math.random() * 150);
      const n = fold(q.trim());
      if (!n) return [];
      const idx = tmdbIndex(db);
      const hits = tmdb.filter((c) => fold(c.title).indexOf(n) >= 0 && (media === "all" || c.media_type === media)).map(cardOf);
      // Library titles also exist on "TMDB".
      db.byId.forEach((r) => {
        if ((r.summary.kind === "movie" || r.summary.kind === "show") && fold(r.summary.title).indexOf(n) >= 0 && !hits.some((h) => h.tmdb_id === r.tmdb_id)) {
          hits.push({
            tmdb_id: r.tmdb_id, media_type: r.summary.kind === "show" ? "tv" : "movie", title: r.summary.title, year: r.summary.year,
            poster_url: r.summary.poster_url, backdrop_url: r.summary.backdrop_url, overview: r.overview, status: idx.has(r.tmdb_id) ? "available" : "none",
            library_item_id: idx.get(r.tmdb_id),
            vote_average: r.rating ?? null, progress: null,
          });
        }
      });
      return hits.slice(0, 30);
    },
    async discoverDetail(media, tid) {
      await latency();
      const c = tmdb.find((x) => x.tmdb_id === tid && x.media_type === media);
      if (c) return { card: cardOf(c), genres: c.genres, runtime_minutes: c.runtime_minutes, seasons: c.seasons, cast: castOfTmdb(c) };
      const lib = Array.from(db.byId.values()).find((r) => r.tmdb_id === tid);
      if (!lib) throw new ApiError(404, "Not found on TMDB");
      return {
        card: { tmdb_id: tid, media_type: media, title: lib.summary.title, year: lib.summary.year, poster_url: lib.summary.poster_url, backdrop_url: lib.summary.backdrop_url, overview: lib.overview, status: "available", vote_average: lib.rating ?? null, progress: null, library_item_id: lib.summary.id },
        genres: lib.genres, runtime_minutes: lib.runtime_s ? Math.round(lib.runtime_s / 60) : null, seasons: lib.childIds.map((_c, i) => i + 1),
        cast: lib.cast,
      };
    },
    async person(pid) {
      await latency();
      return need(mockPerson(db, tmdb, pid, cardOf), "Person");
    },
    async reviews(media, tid) {
      await sleep(250 + Math.random() * 300);
      const lib = Array.from(db.byId.values()).find((r) => r.tmdb_id === tid);
      const cat = tmdb.find((x) => x.tmdb_id === tid);
      return mockReviews(media, tid, lib?.rating ?? cat?.vote_average);
    },
    async downloads() {
      await sleep(60);
      return downloadList();
    },
    async requests(o = {}) {
      await latency();
      return requests.filter((r) => !o.status || r.status === o.status).map((r) => ({ ...r }));
    },
    async activeRequests() {
      return Array.from(sims.values()).map(progressDto);
    },
    async createRequest(b) {
      await sleep(350);
      const c = tmdb.find((x) => x.tmdb_id === b.tmdb_id && x.media_type === b.media_type);
      if (!c) throw new ApiError(404, "Not found on TMDB");
      if (c.status === "available") throw new ApiError(409, "Already available");
      if (requests.some((r) => r.tmdb_id === b.tmdb_id && r.status !== "declined" && r.status !== "failed" && r.status !== "available")) throw new ApiError(409, "Already requested");
      const s = addRequest(c, { id: 1, name: "rafik" }, "approved");
      if (b.seasons) s.req.seasons = b.seasons;
      s.req.created_at = nowS();
      emit(s);
      return { ...s.req };
    },
    async approveRequest(rid) {
      await latency();
      const r = need(requests.find((x) => x.id === rid), "Request");
      if (r.status !== "requested") throw new ApiError(409, "Not pending");
      const s: Sim = { req: r, progress: 0, t: 0 };
      sims.set(r.id, s);
      setStatus(s, "approved");
      r.approved_by = 1;
      emit(s);
      return { ...r };
    },
    async declineRequest(rid, note) {
      await latency();
      const r = need(requests.find((x) => x.id === rid), "Request");
      r.status = "declined";
      r.note = note ?? null;
      const c = tmdb.find((x) => x.tmdb_id === r.tmdb_id);
      if (c) c.status = "none";
      return { ...r };
    },
    async retryRequest(rid) {
      await latency();
      const r = need(requests.find((x) => x.id === rid), "Request");
      const s: Sim = { req: r, progress: 0, t: 0 };
      sims.set(r.id, s);
      setStatus(s, "searching");
      emit(s);
      return { ...r };
    },

    // Admin
    users: async () => (await latency(), users.map((u) => ({ ...u }))),
    async createUser(b) {
      await latency();
      if (users.some((u) => u.username === b.username)) throw new ApiError(409, "Username taken");
      const u: UserDto = { id: users.length + 1, username: b.username, role: b.role ?? "user", disabled: false, library_ids: b.library_ids ?? [], created_at: nowS(), limits: lim(b.limits ?? {}) };
      users.push(u);
      return u;
    },
    async patchUser(uid, b) {
      await latency();
      const u = need(users.find((x) => x.id === uid), "User");
      if (b.disabled != null) u.disabled = b.disabled;
      if (b.role) u.role = b.role;
      if (b.library_ids) u.library_ids = b.library_ids;
      if (b.limits) u.limits = b.limits;
      if (b.username) u.username = b.username;
      return { ...u };
    },
    async deleteUser(uid) {
      await latency();
      const i = users.findIndex((x) => x.id === uid);
      if (i >= 0) users.splice(i, 1);
    },
    revokeUserSessions: ok,
    invites: async () => (await latency(), invites.map((i) => ({ ...i }))),
    async createInvite(b) {
      await latency();
      const code = Math.random().toString(36).slice(2, 6).toUpperCase() + "-" + Math.random().toString(36).slice(2, 6).toUpperCase();
      const inv: InviteDto = { id: invites.length + 1, code, status: "pending", url: `${location.origin}/invite/${code}`, library_ids: b.library_ids ?? [], limits: lim(b.limits ?? {}), expires_at: b.expires_in_days ? nowS() + b.expires_in_days * 86400 : null };
      invites.unshift(inv);
      return inv;
    },
    async deleteInvite(iid) {
      await latency();
      const i = invites.findIndex((x) => x.id === iid);
      if (i >= 0) invites.splice(i, 1);
    },
    arrServers: async () => (await latency(), arr.map((a) => ({ ...a }))),
    async createArrServer(b) {
      await sleep(500);
      if (!/^https?:\/\//.test(b.url)) throw new ApiError(400, "URL must start with http:// or https://");
      const a: ArrServer = { id: arr.length + 1, kind: b.kind, name: b.name, url: b.url, api_key_hint: "••••" + b.api_key.slice(-4), is_4k: !!b.is_4k, default_profile: b.default_profile ?? null, default_root: b.default_root ?? null };
      arr.push(a);
      return a;
    },
    async updateArrServer(aid, b) {
      await latency();
      const a = need(arr.find((x) => x.id === aid), "Server");
      const patch = b as Record<string, unknown>;
      for (const k in patch) if (patch[k] != null && k !== "api_key") (a as unknown as Record<string, unknown>)[k] = patch[k];
      return { ...a };
    },
    async deleteArrServer(aid) {
      const i = arr.findIndex((x) => x.id === aid);
      if (i >= 0) arr.splice(i, 1);
    },
    async arrProfiles(aid) {
      await sleep(600);
      const a = need(arr.find((x) => x.id === aid), "Server");
      if (/fail|bad/.test(a.url)) throw new ApiError(502, "Couldn't reach Radarr: connection refused");
      return {
        quality_profiles: [{ id: 1, name: "Any" }, { id: 4, name: "HD-1080p" }, { id: 5, name: "Ultra-HD" }, { id: 6, name: "HD - 720p/1080p" }],
        root_folders: [{ id: 1, path: a.kind === "sonarr" ? "/media/series" : "/media/films", free_space: 2.4e12 }, { id: 2, path: "/media/4k", free_space: 8.1e11 }],
      };
    },
    downloadClients: async () => (await latency(), dlc.map((d) => ({ ...d }))),
    async createDownloadClient(b) {
      await latency();
      const d: DownloadClient = { id: dlc.length + 1, kind: b.kind, url: b.url, username: b.username ?? null, password_set: !!b.password };
      dlc.push(d);
      return d;
    },
    async deleteDownloadClient(did) {
      const i = dlc.findIndex((x) => x.id === did);
      if (i >= 0) dlc.splice(i, 1);
    },
    notifications: async () => (await latency(), notes.map((n) => ({ ...n }))),
    async createNotification(b) {
      await latency();
      const n: NotificationTarget = { id: notes.length + 1, kind: b.kind, config: b.config, events: b.events ?? [] };
      notes.push(n);
      return n;
    },
    async deleteNotification(nid) {
      const i = notes.findIndex((x) => x.id === nid);
      if (i >= 0) notes.splice(i, 1);
    },
    async testNotification(nid) {
      await sleep(600);
      const n = need(notes.find((x) => x.id === nid), "Target");
      if (n.kind === "webhook") throw new ApiError(502, "Webhook returned HTTP 404");
    },
    sessions: async () => (await latency(), Array.from(playback.values()).map((s) => ({ ...s }))),
    async killSession(sid) {
      await latency();
      playback.delete(sid);
    },
    async hwaccel() {
      await latency();
      return {
        selected: { type: "vaapi", device: "/dev/dri/renderD128", low_power: { h264: true, hevc: true } },
        available: [{ type: "vaapi", device: "/dev/dri/renderD128", low_power: { h264: true, hevc: true } }, { type: "qsv", device: "/dev/dri/renderD128" }, { type: "software" }],
        degraded: ["AV1 encode not supported by this GPU"],
        ffmpeg_hwaccels: ["vaapi", "qsv", "drm", "opencl", "vulkan"],
        notes: ["Intel UHD Graphics 770 (iHD driver 24.1)", "HDR → SDR tone mapping via tonemap_vaapi"],
      };
    },
  };
  return api;
}

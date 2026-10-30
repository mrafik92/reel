import type { Home, ItemDetail, ItemSummary, Library, LibraryKind, LibrarySort, Page, Id } from "@reel/ui";
import { getServerUrl, onServerUrlChange, setApiBase } from "@reel/ui";
import { MOCK } from "../lib/mode";
import { http } from "./http";
import type {
  AdminSession, ArrProfiles, ArrServer, CreateArrServer, CreateDownloadClient, CreateInvite, CreateLibrary,
  CreateNotification, CreateRequest, CreateUser, DecideRequest, DecideResponse, DiscoverCard, DiscoverDetail,
  DiscoverList, DiscoverPage, DownloadClient, HwAccelStatus, InviteDto, InviteInfo, LoginResponse, Me, MediaType,
  NotificationTarget, PatchUser, ProgressDto, RequestDto, RequestState, SetupStatus, UpdateArrServer, UserDto,
  DiscoverDetailWithCast, PersonDetail, Reviews, DownloadItem, GenreCount, LibraryFilters, BrowseLane, SearchAll,
} from "./types";

export { ApiError, getToken, setToken, onUnauthorized } from "./http";
export type * from "./types";
export type { Home, ItemDetail, ItemSummary, Library, LibraryKind, LibrarySort, Page, Id };

export interface Credentials {
  username: string;
  password: string;
  device_id?: string;
  device_name?: string;
  client?: string;
}

/** Typed access to every endpoint the web client uses. HTTP by default, fixtures with `?mock=1`. */
export interface Api {
  readonly mock: boolean;

  // Auth
  setupStatus(): Promise<SetupStatus>;
  setup(c: Credentials): Promise<LoginResponse>;
  login(c: Credentials): Promise<LoginResponse>;
  logout(): Promise<void>;
  me(): Promise<Me>;
  inviteInfo(code: string): Promise<InviteInfo>;
  acceptInvite(code: string, c: Credentials): Promise<LoginResponse>;

  // Library
  libraries(): Promise<Library[]>;
  createLibrary(b: CreateLibrary): Promise<Library>;
  scanLibrary(id: Id): Promise<void>;
  libraryItems(id: Id, opts: { sort: LibrarySort; cursor?: string | null; limit?: number; seed?: number; filters?: LibraryFilters }): Promise<Page<ItemSummary>>;
  /** Genres with item counts (whole server, or one library). */
  genres(libraryId?: Id): Promise<GenreCount[]>;
  /** Curated picker lanes for one library. */
  browseLanes(libraryId: Id): Promise<BrowseLane[]>;
  /** One random title matching the filters (404 when nothing does). */
  pick(libraryId: Id, filters?: LibraryFilters): Promise<ItemSummary>;
  /** Library + episodes + people + TMDB in one go. */
  searchAll(q: string, limit?: number): Promise<SearchAll>;
  item(id: Id): Promise<ItemDetail>;
  /** Plain array (not paged). */
  children(id: Id): Promise<ItemSummary[]>;
  home(): Promise<Home>;
  /** Library search — movies + shows only, plain array. */
  search(q: string, limit?: number): Promise<ItemSummary[]>;
  setPlayed(id: Id, played: boolean): Promise<void>;

  // Playback
  decide(fileId: Id, body: DecideRequest): Promise<DecideResponse>;
  progress(sessionId: string, body: { position_s: number; paused?: boolean }, keepalive?: boolean): Promise<void>;
  stopPlayback(sessionId: string, keepalive?: boolean): Promise<void>;

  // Discover & requests
  discover(list: DiscoverList, media?: "movie" | "tv" | "all", page?: number): Promise<DiscoverPage>;
  discoverSearch(q: string, media?: "movie" | "tv" | "all"): Promise<DiscoverCard[]>;
  discoverDetail(media: MediaType, tmdbId: number): Promise<DiscoverDetailWithCast>;
  /** Person + filmography (newest first), each credit with availability. */
  person(tmdbId: number): Promise<PersonDetail>;
  /** Ratings (IMDb/RT/Metacritic/TMDB), reviews, external links. */
  reviews(media: MediaType, tmdbId: number): Promise<Reviews>;
  /** All Radarr/Sonarr queue activity. */
  downloads(): Promise<DownloadItem[]>;
  requests(o?: { status?: RequestState; limit?: number; offset?: number }): Promise<RequestDto[]>;
  activeRequests(): Promise<ProgressDto[]>;
  createRequest(b: CreateRequest): Promise<RequestDto>;
  approveRequest(id: number): Promise<RequestDto>;
  declineRequest(id: number, note?: string): Promise<RequestDto>;
  retryRequest(id: number): Promise<RequestDto>;

  // Admin
  users(): Promise<UserDto[]>;
  createUser(b: CreateUser): Promise<UserDto>;
  patchUser(id: number, b: PatchUser): Promise<UserDto>;
  deleteUser(id: number): Promise<void>;
  revokeUserSessions(id: number): Promise<void>;
  invites(): Promise<InviteDto[]>;
  createInvite(b: CreateInvite): Promise<InviteDto>;
  deleteInvite(id: number): Promise<void>;
  arrServers(): Promise<ArrServer[]>;
  createArrServer(b: CreateArrServer): Promise<ArrServer>;
  updateArrServer(id: number, b: UpdateArrServer): Promise<ArrServer>;
  deleteArrServer(id: number): Promise<void>;
  arrProfiles(id: number): Promise<ArrProfiles>;
  downloadClients(): Promise<DownloadClient[]>;
  createDownloadClient(b: CreateDownloadClient): Promise<DownloadClient>;
  deleteDownloadClient(id: number): Promise<void>;
  notifications(): Promise<NotificationTarget[]>;
  createNotification(b: CreateNotification): Promise<NotificationTarget>;
  deleteNotification(id: number): Promise<void>;
  testNotification(id: number): Promise<void>;
  sessions(): Promise<AdminSession[]>;
  killSession(id: string): Promise<void>;
  hwaccel(): Promise<HwAccelStatus>;
}

// Media URLs are server-relative and pre-signed; only the origin is prefixed (TV builds run from file://).
setApiBase(getServerUrl());
onServerUrlChange((u) => setApiBase(u));

const noAuth = { auth: false };

/** LibraryFilters → wire query (omits empty values). */
export function filterQuery(f?: LibraryFilters): Record<string, string | number | boolean | undefined> {
  if (!f) return {};
  return {
    genre: f.genre && f.genre.length ? f.genre.join(",") : undefined,
    year_from: f.year_from,
    year_to: f.year_to,
    min_rating: f.min_rating,
    max_runtime_min: f.max_runtime_min,
    unwatched: f.unwatched ? true : undefined,
  };
}

const httpApi: Api = {
  mock: false,

  setupStatus: () => http.get("/setup", undefined, noAuth),
  setup: (c) => http.post("/setup", c, noAuth),
  login: (c) => http.post("/auth/login", c, noAuth),
  logout: () => http.post("/auth/logout"),
  me: () => http.get("/me"),
  inviteInfo: (code) => http.get(`/auth/invite/${encodeURIComponent(code)}`, undefined, noAuth),
  acceptInvite: (code, c) => http.post(`/auth/invite/${encodeURIComponent(code)}/accept`, c, noAuth),

  libraries: () => http.get("/libraries"),
  createLibrary: (b) => http.post("/libraries", b),
  scanLibrary: (id) => http.post(`/libraries/${id}/scan`),
  // Only this endpoint is paged; `next_cursor` is opaque.
  libraryItems: (id, o) =>
    http.get(`/libraries/${id}/items`, { sort: o.sort, cursor: o.cursor, limit: o.limit ?? 60, seed: o.sort === "random" ? o.seed : undefined, ...filterQuery(o.filters) }),
  genres: (libraryId) => http.get("/genres", { library_id: libraryId }),
  browseLanes: (libraryId) => http.get("/browse/lanes", { library_id: libraryId }),
  pick: (libraryId, f) => http.get("/pick", { library_id: libraryId, ...filterQuery(f) }),
  searchAll: (q, limit = 30) => http.get("/search/all", { q, limit }),
  item: (id) => http.get(`/items/${id}`),
  children: (id) => http.get(`/items/${id}/children`),
  home: () => http.get("/home"),
  search: (q, limit = 60) => http.get("/search", { q, limit }),
  setPlayed: (id, played) => (played ? http.post(`/items/${id}/played`) : http.del(`/items/${id}/played`)),

  decide: (fileId, body) => http.post(`/playback/${fileId}/decide`, body),
  progress: (sid, body, keepalive) => http.post(`/playback/${encodeURIComponent(sid)}/progress`, body, { keepalive }),
  stopPlayback: (sid, keepalive) => http.del(`/playback/${encodeURIComponent(sid)}`, { keepalive }),

  discover: (list, media = "all", page = 1) => http.get(`/discover/${list}`, { media, page }),
  discoverSearch: (q, media = "all") => http.get("/discover/search", { q, media }),
  discoverDetail: (media, id) => http.get(`/discover/${media}/${id}`),
  person: (id) => http.get(`/people/${id}`),
  reviews: (media, id) => http.get(`/reviews/${media}/${id}`),
  downloads: () => http.get("/downloads"),
  requests: (o = {}) => http.get("/requests", { status: o.status, limit: o.limit, offset: o.offset }),
  activeRequests: () => http.get("/requests/active"),
  createRequest: (b) => http.post("/requests", b),
  approveRequest: (id) => http.post(`/requests/${id}/approve`),
  declineRequest: (id, note) => http.post(`/requests/${id}/decline`, { note: note ?? null }),
  retryRequest: (id) => http.post(`/requests/${id}/retry`),

  users: () => http.get("/admin/users"),
  createUser: (b) => http.post("/admin/users", b),
  patchUser: (id, b) => http.patch(`/admin/users/${id}`, b),
  deleteUser: (id) => http.del(`/admin/users/${id}`),
  revokeUserSessions: (id) => http.del(`/admin/users/${id}/sessions`),
  invites: () => http.get("/admin/invites"),
  createInvite: (b) => http.post("/admin/invites", b),
  deleteInvite: (id) => http.del(`/admin/invites/${id}`),
  arrServers: () => http.get("/admin/arr-servers"),
  createArrServer: (b) => http.post("/admin/arr-servers", b),
  updateArrServer: (id, b) => http.patch(`/admin/arr-servers/${id}`, b),
  deleteArrServer: (id) => http.del(`/admin/arr-servers/${id}`),
  arrProfiles: (id) => http.get(`/admin/arr-servers/${id}/profiles`),
  downloadClients: () => http.get("/admin/download-clients"),
  createDownloadClient: (b) => http.post("/admin/download-clients", b),
  deleteDownloadClient: (id) => http.del(`/admin/download-clients/${id}`),
  notifications: () => http.get("/admin/notifications"),
  createNotification: (b) => http.post("/admin/notifications", b),
  deleteNotification: (id) => http.del(`/admin/notifications/${id}`),
  testNotification: (id) => http.post(`/admin/notifications/${id}/test`),
  sessions: () => http.get("/admin/sessions"),
  killSession: (id) => http.del(`/admin/sessions/${encodeURIComponent(id)}`),
  hwaccel: () => http.get("/admin/hwaccel"),
};

let impl: Promise<Api> | undefined;
/** The concrete implementation (mock fixtures are code-split out of production). */
export function backend(): Promise<Api> {
  if (!impl) impl = MOCK ? import("./mock").then((m) => m.createMockApi()) : Promise.resolve(httpApi);
  return impl;
}

// Short-lived read cache so Back navigation renders instantly. Writes invalidate.
const cache = new Map<string, { at: number; p: Promise<unknown> }>();
const TTL = 30_000;
function cached<T>(key: string, fn: () => Promise<T>, ttl = TTL): Promise<T> {
  const hit = cache.get(key);
  if (hit && Date.now() - hit.at < ttl) return hit.p as Promise<T>;
  const p = fn();
  cache.set(key, { at: Date.now(), p });
  p.catch(() => cache.delete(key));
  return p;
}
export function invalidate(prefix = ""): void {
  cache.forEach((_v, k) => {
    if (k.indexOf(prefix) === 0) cache.delete(k);
  });
}

/** Proxy every call to the backend; reads listed in READS go through the cache. */
const READS: Partial<Record<keyof Api, number>> = {
  libraries: TTL, libraryItems: TTL, item: TTL, children: TTL, home: TTL, search: TTL,
  genres: 5 * 60_000, browseLanes: 60_000, searchAll: 60_000,
  discover: 5 * 60_000, discoverDetail: 5 * 60_000, discoverSearch: 60_000,
  person: 10 * 60_000, reviews: 30 * 60_000,
};

function wrap(): Api {
  const out: Record<string, unknown> = { mock: MOCK };
  (Object.keys(httpApi) as (keyof Api)[]).forEach((k) => {
    if (k === "mock") return;
    out[k] = (...args: unknown[]) => {
      const run = () => backend().then((a) => (a[k] as unknown as (...x: unknown[]) => Promise<unknown>)(...args));
      const ttl = READS[k];
      return ttl ? cached(`${k}:${JSON.stringify(args)}`, run, ttl) : run();
    };
  });
  return out as unknown as Api;
}

export const api: Api = wrap();

/** Mutations that change library state should call this so lists refresh. */
export function invalidateLibrary(): void {
  ["libraries", "libraryItems", "item", "children", "home", "search", "genres", "browseLanes", "searchAll"].forEach((k) => invalidate(k));
}


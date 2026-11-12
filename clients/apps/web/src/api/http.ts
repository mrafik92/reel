import { serverUrl } from "@reel/ui";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    /** Seconds, from `Retry-After` (429 lockouts and limits). */
    public retryAfter?: number,
    /** Parsed JSON error body, when the server sent one (extra fields beyond `error`). */
    public body?: Record<string, unknown>,
  ) {
    super(message);
  }
}

const TOKEN_KEY = "reel.token";
let token: string | null = null;
try {
  token = localStorage.getItem(TOKEN_KEY);
} catch {
  /* storage disabled */
}

export function getToken(): string | null {
  return token;
}
export function setToken(t: string | null): void {
  token = t;
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore */
  }
}

let unauthorized: (() => void) | undefined;
/** Called on any 401 from an authenticated route (session expired or revoked). */
export function onUnauthorized(fn: () => void): void {
  unauthorized = fn;
}

export type Query = Record<string, string | number | boolean | null | undefined>;

export interface RequestOptions {
  query?: Query;
  body?: unknown;
  /** Send the bearer token (default true). */
  auth?: boolean;
  /** Survive page unload (playback stop). */
  keepalive?: boolean;
}

function qs(q?: Query): string {
  if (!q) return "";
  const p = new URLSearchParams();
  for (const k in q) {
    const v = q[k];
    if (v != null && v !== "") p.set(k, String(v));
  }
  const s = p.toString();
  return s ? "?" + s : "";
}

const FRIENDLY: Record<number, string> = {
  0: "Can't reach your Reel server",
  401: "Your session has ended — please sign in again",
  403: "You don't have access to that",
  404: "Not found",
  409: "That already exists",
  424: "The server isn't fully set up for this yet",
  429: "Too many requests — try again shortly",
  502: "The server couldn't reach an external service",
};

// ---- connection health (#81) -------------------------------------------------------------------
// Pages often swallow a failed read (a lane that `.catch()`es to empty) and would stay empty after
// a network blip. When a page's read failed for network reasons and a later request gets through,
// listeners (the app shell) re-mount the page so everything loads again.

/** Background polls and keepalives: their failures don't leave a page half-loaded. */
const BACKGROUND = /^\/(downloads|requests\/active|auth\/ws-ticket|playback\/|live\/|iptv\/sessions\/)/;
let pageReadFailed = false;
const restoredHandlers = new Set<() => void>();

/** Called once the server answers again after a page's read failed for network reasons. */
export function onConnectionRestored(fn: () => void): () => void {
  restoredHandlers.add(fn);
  return () => restoredHandlers.delete(fn);
}
/** A page's read failed for network reasons since the last restore. */
export function hadNetworkFailure(): boolean {
  return pageReadFailed;
}
function noteResult(method: string, path: string, reached: boolean): void {
  if (!reached) {
    if (method === "GET" && !BACKGROUND.test(path)) pageReadFailed = true;
    return;
  }
  if (!pageReadFailed) return;
  pageReadFailed = false;
  restoredHandlers.forEach((h) => h());
}

/** A request that hangs this long (a wedged HTTP/3 connection, a frozen tab) fails instead (#81).
 *  Writes get longer: tuning a channel or starting playback can take a while on the server. */
const READ_TIMEOUT_MS = 20_000;
const WRITE_TIMEOUT_MS = 60_000;

/** One fetch with a timeout. Network failures, aborts and timeouts all become status 0. */
async function send(url: string, init: RequestInit): Promise<Response> {
  const ctl = typeof AbortController !== "undefined" ? new AbortController() : undefined;
  const ms = init.method === "GET" ? READ_TIMEOUT_MS : WRITE_TIMEOUT_MS;
  const timer = ctl ? setTimeout(() => ctl.abort(), ms) : undefined;
  try {
    return await fetch(url, { ...init, signal: ctl?.signal });
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export async function request<T>(method: string, path: string, o: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (o.body !== undefined) headers["Content-Type"] = "application/json";
  if (o.auth !== false && token) headers.Authorization = `Bearer ${token}`;
  // serverUrl(): same origin on the web, the configured address in file:// TV builds.
  const url = serverUrl(`/api/v1${path}${qs(o.query)}`);
  const init: RequestInit = {
    method,
    headers,
    body: o.body !== undefined ? JSON.stringify(o.body) : undefined,
    keepalive: o.keepalive,
  };
  let res: Response | undefined;
  // Reads are retried once after a network blip (a reset connection, a request the browser dropped
  // while the tab was in the background); writes aren't, they may already have happened.
  const tries = method === "GET" ? 2 : 1;
  for (let i = 0; i < tries && !res; i++) {
    if (i) await new Promise((r) => setTimeout(r, 600));
    try {
      res = await send(url, init);
    } catch {
      /* status 0 below */
    }
  }
  noteResult(method, path, !!res);
  if (!res) throw new ApiError(0, FRIENDLY[0]!);
  if (!res.ok) {
    let msg = "";
    let body: Record<string, unknown> | undefined;
    try {
      const b = (await res.json()) as { error?: string };
      msg = b && b.error ? b.error : "";
      if (b && typeof b === "object") body = b as Record<string, unknown>;
    } catch {
      /* not JSON */
    }
    const ra = Number(res.headers.get("Retry-After"));
    if (res.status === 401 && o.auth !== false) unauthorized?.();
    throw new ApiError(res.status, msg || FRIENDLY[res.status] || `Server error (${res.status})`, isFinite(ra) && ra > 0 ? ra : undefined, body);
  }
  if (res.status === 204 || res.status === 202) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export const http = {
  get: <T>(p: string, query?: Query, o?: RequestOptions) => request<T>("GET", p, { ...o, query }),
  post: <T>(p: string, body?: unknown, o?: RequestOptions) => request<T>("POST", p, { ...o, body }),
  patch: <T>(p: string, body?: unknown, o?: RequestOptions) => request<T>("PATCH", p, { ...o, body }),
  del: <T>(p: string, o?: RequestOptions) => request<T>("DELETE", p, o),
};

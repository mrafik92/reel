import { serverUrl } from "@reel/ui";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    /** Seconds, from `Retry-After` (429 lockouts and limits). */
    public retryAfter?: number,
    /** Parsed JSON error body, when the server sent one (extra fields beyond `error`). */
    public body?: Record<string, unknown>,
    /** Why a request never got an answer (status 0 only). */
    public network?: NetworkFailure,
  ) {
    super(message);
  }
}

/**
 * Why a request got no answer (#90):
 * - `aborted`: the caller cancelled it or the page is unloading. Nothing went wrong; don't show it.
 * - `offline`: the browser says it has no network.
 * - `timeout`: no answer within the time limit.
 * - `network`: the connection failed (server down, connection reset, a route blip).
 */
export type NetworkFailure = "aborted" | "offline" | "timeout" | "network";

/** A request the caller or the page cancelled: not an error worth showing. */
export function isAborted(e: unknown): boolean {
  return e instanceof ApiError && e.status === 0 && e.network === "aborted";
}

/** Classify a failed fetch. Pure, so it's unit-tested. */
export function classifyFailure(f: { callerAborted: boolean; unloading: boolean; timedOut: boolean; online: boolean }): NetworkFailure {
  if (f.callerAborted || f.unloading) return "aborted";
  if (!f.online) return "offline";
  if (f.timedOut) return "timeout";
  return "network";
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
  /** Cancel the request (page left, a newer request superseded it). Fails with an `aborted` ApiError. */
  signal?: AbortSignal;
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
  0: "Can't reach Reel",
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

const NETWORK_MESSAGE: Record<NetworkFailure, string> = {
  aborted: "Request cancelled",
  offline: "You're offline",
  timeout: "Reel took too long to answer",
  network: FRIENDLY[0]!,
};

// Requests the browser cancels while the page unloads (reload, leaving the site) aren't failures.
let unloading = false;
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  window.addEventListener("pagehide", () => (unloading = true));
  window.addEventListener("pageshow", () => (unloading = false));
}

const isOnline = () => typeof navigator === "undefined" || navigator.onLine !== false;

/** One fetch with a timeout. Throws the `NetworkFailure` when no response arrived. */
async function send(url: string, init: RequestInit, signal?: AbortSignal): Promise<Response> {
  if (signal?.aborted) throw "aborted" satisfies NetworkFailure;
  const ctl = typeof AbortController !== "undefined" ? new AbortController() : undefined;
  const ms = init.method === "GET" ? READ_TIMEOUT_MS : WRITE_TIMEOUT_MS;
  let timedOut = false;
  const timer = ctl
    ? setTimeout(() => {
        timedOut = true;
        ctl.abort();
      }, ms)
    : undefined;
  const onAbort = () => ctl?.abort();
  signal?.addEventListener("abort", onAbort);
  try {
    return await fetch(url, { ...init, signal: ctl?.signal });
  } catch {
    throw classifyFailure({ callerAborted: !!signal?.aborted, unloading, timedOut, online: isOnline() });
  } finally {
    if (timer) clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
  }
}

/** Pause between the first try of a read and its automatic retry. */
const RETRY_DELAY_MS = 600;

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
  let failure: NetworkFailure = "network";
  // Reads are retried once after a network blip (a reset connection, a request the browser dropped
  // while the tab was in the background); writes aren't, they may already have happened. Cancelled
  // requests aren't retried.
  const tries = method === "GET" ? 2 : 1;
  for (let i = 0; i < tries && !res && failure !== "aborted"; i++) {
    if (i) await new Promise((r) => setTimeout(r, RETRY_DELAY_MS));
    try {
      res = await send(url, init, o.signal);
    } catch (e) {
      failure = typeof e === "string" ? (e as NetworkFailure) : "network";
      if (o.signal?.aborted || unloading) failure = "aborted";
    }
  }
  if (!res) {
    // A cancelled request says nothing about the connection.
    if (failure !== "aborted") noteResult(method, path, false);
    throw new ApiError(0, NETWORK_MESSAGE[failure], undefined, undefined, failure);
  }
  noteResult(method, path, true);
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

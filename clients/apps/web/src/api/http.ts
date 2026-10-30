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

export async function request<T>(method: string, path: string, o: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (o.body !== undefined) headers["Content-Type"] = "application/json";
  if (o.auth !== false && token) headers.Authorization = `Bearer ${token}`;
  let res: Response;
  try {
    // serverUrl(): same origin on the web, the configured address in file:// TV builds.
    res = await fetch(serverUrl(`/api/v1${path}${qs(o.query)}`), {
      method,
      headers,
      body: o.body !== undefined ? JSON.stringify(o.body) : undefined,
      keepalive: o.keepalive,
    });
  } catch {
    throw new ApiError(0, FRIENDLY[0]!);
  }
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

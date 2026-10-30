/**
 * Server base URL store. Packaged TV apps load from file:// so they have no "same origin" —
 * the user enters the server address on first run. On the web the default is same origin ("").
 */
const KEY = "reel.serverUrl";
const listeners = new Set<(url: string) => void>();

function storage(): Storage | undefined {
  try {
    return typeof localStorage !== "undefined" ? localStorage : undefined;
  } catch {
    return undefined;
  }
}

/** Build-time default (VITE_REEL_URL), if any. */
function buildDefault(): string {
  const env = (import.meta as any).env;
  return normalizeServerUrl(env?.VITE_REEL_URL ?? "") ?? "";
}

/** Normalise user input: adds http:// if no scheme, strips trailing slashes. Returns null if unusable. */
export function normalizeServerUrl(input: string): string | null {
  let s = (input ?? "").trim();
  if (!s) return "";
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) s = "http://" + s;
  try {
    const u = new URL(s);
    if (u.protocol !== "http:" && u.protocol !== "https:") return null;
    return (u.origin + u.pathname).replace(/\/+$/, "");
  } catch {
    return null;
  }
}

/** True when running from a packaged file:// bundle (no usable same origin). */
export function isFileOrigin(): boolean {
  return typeof location !== "undefined" && location.protocol === "file:";
}

/** Stored server URL, else build default, else "" (same origin). */
export function getServerUrl(): string {
  const stored = storage()?.getItem(KEY);
  if (stored) return stored;
  return buildDefault();
}

/** True if the app can't reach a server yet (file:// origin and nothing configured) → show the setup screen. */
export function needsServerSetup(): boolean {
  return isFileOrigin() && !getServerUrl();
}

/** Persist the server URL. Returns the normalised value, or null if invalid (nothing stored). Empty clears it. */
export function setServerUrl(input: string): string | null {
  const url = normalizeServerUrl(input);
  if (url === null) return null;
  const s = storage();
  if (url === "") s?.removeItem(KEY);
  else s?.setItem(KEY, url);
  listeners.forEach((l) => l(url));
  return url;
}

export function clearServerUrl(): void {
  setServerUrl("");
}

export function onServerUrlChange(h: (url: string) => void): () => void {
  listeners.add(h);
  return () => listeners.delete(h);
}

/** Resolve an API/media path against the configured server. */
export function serverUrl(path: string): string {
  return getServerUrl() + (path.startsWith("/") ? path : "/" + path);
}

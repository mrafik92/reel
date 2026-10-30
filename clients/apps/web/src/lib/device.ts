/**
 * Device identity sent with login/setup and playback decisions.
 *
 * TV builds set the client with `VITE_CLIENT=webos|tizen` at build time, or the
 * platform layer can call `setClientInfo()` before the app renders.
 */
import { clientName } from "@reel/ui";

export type ClientKind = "web" | "webos" | "tizen" | (string & {});

interface ClientInfo {
  client: ClientKind;
  deviceName: string;
  /** Server-side device profile preset (see DecideRequest.preset). */
  preset: string;
}

function detectClient(): ClientKind {
  // Build-time flag wins; otherwise the platform layer's runtime detection.
  return import.meta.env.VITE_CLIENT || clientName;
}

function detectName(client: ClientKind): string {
  if (client === "webos") return "LG TV";
  if (client === "tizen") return "Samsung TV";
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox"
    : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
  const os = /Mac OS X/.test(ua) ? "macOS" : /Windows/.test(ua) ? "Windows" : /Android/.test(ua) ? "Android"
    : /iPhone|iPad/.test(ua) ? "iOS" : /Linux/.test(ua) ? "Linux" : "";
  return os ? `${browser} on ${os}` : browser;
}

function presetFor(client: ClientKind): string {
  if (client === "webos") return "webos";
  if (client === "tizen") return "tizen";
  const ua = navigator.userAgent;
  return /Safari\//.test(ua) && !/Chrome\/|Chromium\//.test(ua) ? "web_safari" : "web_chrome";
}

const initial = detectClient();
let info: ClientInfo = { client: initial, deviceName: detectName(initial), preset: presetFor(initial) };

/** Hook for platform code (webOS/Tizen shells) to override what we report. */
export function setClientInfo(p: Partial<ClientInfo>): void {
  info = { ...info, ...p };
  if (p.client && !p.preset) info.preset = presetFor(p.client);
}
export function clientInfo(): Readonly<ClientInfo> {
  return info;
}

function uuid(): string {
  const b = new Uint8Array(16);
  if (typeof crypto !== "undefined" && crypto.getRandomValues) crypto.getRandomValues(b);
  else for (let i = 0; i < 16; i++) b[i] = Math.floor(Math.random() * 256);
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const h: string[] = [];
  b.forEach((x) => h.push((x + 0x100).toString(16).slice(1)));
  return `${h.slice(0, 4).join("")}-${h.slice(4, 6).join("")}-${h.slice(6, 8).join("")}-${h.slice(8, 10).join("")}-${h.slice(10).join("")}`;
}

let cachedId: string | undefined;
/** Stable per-install id, persisted in localStorage. */
export function deviceId(): string {
  if (cachedId) return cachedId;
  try {
    cachedId = localStorage.getItem("reel.device_id") || undefined;
    if (!cachedId) {
      cachedId = uuid();
      localStorage.setItem("reel.device_id", cachedId);
    }
  } catch {
    cachedId = cachedId || uuid();
  }
  return cachedId;
}

/** Fields the auth endpoints accept to label this device in "Active sessions". */
export function deviceFields() {
  return { device_id: deviceId(), device_name: info.deviceName, client: String(info.client) };
}

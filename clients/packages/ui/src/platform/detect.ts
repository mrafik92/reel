/// <reference path="./webos.d.ts" />
/// <reference path="./tizen.d.ts" />

export type Platform = "webos" | "tizen" | "android-tv" | "web";
export type ClientName = "webos" | "tizen" | "web" | "android-web";

/** What the Reel Web Android shell (#226) tells us through its user-agent token. */
export interface ReelShell {
  /** The APK's versionName. */
  version: string;
  /** Android's UI mode is television (the token ends in ` (TV)`). */
  tv: boolean;
}

/**
 * The Reel Web Android shell appends `ReelAndroidWeb/<versionName>` to the WebView's user agent,
 * plus ` (TV)` on Android TV. The shell has no JavaScript bridge: this token is its only signal.
 */
const currentUa = (): string => (typeof navigator !== "undefined" ? navigator.userAgent : "");

export function reelShell(ua = currentUa()): ReelShell | null {
  const m = /\bReelAndroidWeb\/([^\s()]+)( \(TV\))?/.exec(ua);
  return m ? { version: m[1]!, tv: !!m[2] } : null;
}

/**
 * Device model from an Android user agent: `(Linux; Android 14; Pixel 8 Build/UQ1A…; wv)` → "Pixel 8".
 * Chrome's reduced user agent says `Android 10; K` — no model, so that gives null.
 */
export function androidModel(ua: string): string | null {
  const m = /\(Linux;(?: U;)? Android [^;)]*;\s*([^;)]+?)\s*(?:Build\/[^;)]*)?(?:;|\))/.exec(ua);
  const model = m?.[1]?.trim();
  if (!model || model === "K" || model === "wv" || /^[a-z]{2}[-_][a-z]{2}$/i.test(model)) return null;
  return model.slice(0, 40);
}

/** "Reel Web on Pixel 8", or on "Android" / "Android TV" when the user agent has no model. */
export function shellDeviceName(ua: string): string {
  return `Reel Web on ${androidModel(ua) ?? (reelShell(ua)?.tv ? "Android TV" : "Android")}`;
}

/** The label the server shows for this device in signed-in devices. */
export function deviceNameFor(client: string, ua = currentUa()): string {
  if (client === "webos") return "LG TV";
  if (client === "tizen") return "Samsung TV";
  if (client === "android-web") return shellDeviceName(ua);
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox"
    : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
  const os = /Mac OS X/.test(ua) ? "macOS" : /Windows/.test(ua) ? "Windows" : /Android/.test(ua) ? "Android"
    : /iPhone|iPad/.test(ua) ? "iOS" : /Linux/.test(ua) ? "Linux" : "";
  return os ? `${browser} on ${os}` : browser;
}

/** Decide preset: the TV presets, else Safari or Chrome by engine (the Android WebView is Chrome). */
export function presetFor(client: string, ua = currentUa()): string {
  if (client === "webos") return "webos";
  if (client === "tizen") return "tizen";
  if (client === "android-web") return "web_chrome";
  return /Safari\//.test(ua) && !/Chrome\/|Chromium\//.test(ua) ? "web_safari" : "web_chrome";
}

/** Pure detector (takes the global object so tests can fake it). */
export function detectPlatform(g: any = globalThis): Platform {
  const ua: string = g.navigator?.userAgent ?? "";
  if (g.PalmSystem || g.webOS?.platform || /Web0S|webOS/i.test(ua)) return "webos";
  if (g.tizen || /Tizen/i.test(ua)) return "tizen";
  // Inside the Reel Web shell its token decides (a phone model name mustn't flip the 10-foot UI).
  const shell = reelShell(ua);
  if (shell) return shell.tv ? "android-tv" : "web";
  if (/Android/i.test(ua) && /(\bTV\b|AFT[A-Z]|BRAVIA|SHIELD|Android TV)/i.test(ua)) return "android-tv";
  return "web";
}

export function clientNameFor(p: Platform): ClientName {
  return p === "webos" ? "webos" : p === "tizen" ? "tizen" : "web";
}

/** Client identity for the server: the platform's, or `android-web` inside the Reel Web shell. */
export function detectClientName(g: any = globalThis): ClientName {
  const p = detectPlatform(g);
  if ((p === "web" || p === "android-tv") && reelShell(g.navigator?.userAgent ?? "")) return "android-web";
  return clientNameFor(p);
}

/** Runtime platform (evaluated once at import). */
export const platform: Platform = detectPlatform();
/** Name sent to the server as the client identity / preset family. */
export const clientName: ClientName = detectClientName();
/** True on any 10-foot device (webOS, Tizen, Android TV WebView). */
export const isTv: boolean = platform !== "web";
/** Running inside the Reel Web Android shell (#226). */
export const inReelShell: boolean = reelShell() != null;

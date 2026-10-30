/// <reference path="./webos.d.ts" />
/// <reference path="./tizen.d.ts" />

export type Platform = "webos" | "tizen" | "android-tv" | "web";
export type ClientName = "webos" | "tizen" | "web";

/** Pure detector (takes the global object so tests can fake it). */
export function detectPlatform(g: any = globalThis): Platform {
  const ua: string = g.navigator?.userAgent ?? "";
  if (g.PalmSystem || g.webOS?.platform || /Web0S|webOS/i.test(ua)) return "webos";
  if (g.tizen || /Tizen/i.test(ua)) return "tizen";
  if (/Android/i.test(ua) && /(\bTV\b|AFT[A-Z]|BRAVIA|SHIELD|Android TV)/i.test(ua)) return "android-tv";
  return "web";
}

export function clientNameFor(p: Platform): ClientName {
  return p === "webos" ? "webos" : p === "tizen" ? "tizen" : "web";
}

/** Runtime platform (evaluated once at import). */
export const platform: Platform = detectPlatform();
/** Name sent to the server as the client identity / preset family. */
export const clientName: ClientName = clientNameFor(platform);
/** True on any 10-foot device (webOS, Tizen, Android TV WebView). */
export const isTv: boolean = platform !== "web";

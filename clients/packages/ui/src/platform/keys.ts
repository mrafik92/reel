/// <reference path="./webos.d.ts" />
/// <reference path="./tizen.d.ts" />
import { platform } from "./detect";

export type RemoteKey =
  | "up" | "down" | "left" | "right" | "enter" | "back"
  | "play" | "pause" | "playpause" | "stop" | "ff" | "rw"
  | "channelUp" | "channelDown"
  | "red" | "green" | "yellow" | "blue"
  | "info" | "digit";

export interface RemoteKeyEvent {
  key: RemoteKey;
  /** 0-9 when key === "digit". */
  digit?: number;
  original: KeyboardEvent;
}
/** Return true to mark the key handled (preventDefault). */
export type RemoteKeyHandler = (e: RemoteKeyEvent) => boolean | void;

const CODES: Record<number, RemoteKey> = {
  37: "left", 38: "up", 39: "right", 40: "down", 13: "enter",
  461: "back", 10009: "back", 27: "back", 8: "back",
  415: "play", 19: "pause", 10252: "playpause", 179: "playpause",
  413: "stop", 417: "ff", 412: "rw",
  427: "channelUp", 428: "channelDown",
  403: "red", 404: "green", 405: "yellow", 406: "blue",
  457: "info", 10114: "info",
};

const KEYS: Record<string, RemoteKey> = {
  ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right", Enter: "enter",
  Escape: "back", GoBack: "back", BrowserBack: "back", XF86Back: "back", Backspace: "back",
  MediaPlay: "play", MediaPause: "pause", MediaPlayPause: "playpause", MediaStop: "stop",
  MediaFastForward: "ff", MediaRewind: "rw", ChannelUp: "channelUp", ChannelDown: "channelDown",
  ColorF0Red: "red", ColorF1Green: "green", ColorF2Yellow: "yellow", ColorF3Blue: "blue",
  Info: "info",
};

export function normalizeKey(e: { keyCode?: number; key?: string; code?: string }): { key: RemoteKey; digit?: number } | null {
  const k = e.key;
  if (k && /^[0-9]$/.test(k)) return { key: "digit", digit: Number(k) };
  const kc = e.keyCode ?? 0;
  if (kc >= 48 && kc <= 57) return { key: "digit", digit: kc - 48 };
  if (kc >= 96 && kc <= 105) return { key: "digit", digit: kc - 96 };
  // Prefer keyCode: TV remotes often report meaningless `key` values ("Unidentified").
  const byCode = CODES[kc];
  if (byCode) return { key: byCode };
  const byKey = k ? KEYS[k] : undefined;
  return byKey ? { key: byKey } : null;
}

const TIZEN_KEYS = [
  "MediaPlay", "MediaPause", "MediaStop", "MediaFastForward", "MediaRewind", "MediaPlayPause",
  "ChannelUp", "ChannelDown", "ColorF0Red", "ColorF1Green", "ColorF2Yellow", "ColorF3Blue",
  "Info", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

/** Tizen only delivers media/colour/number keys after registration. Idempotent. */
export function registerTizenKeys(g: any = globalThis): void {
  try {
    const tv = g.tizen?.tvinputdevice;
    if (!tv?.registerKeyBatch) return;
    const supported = new Set<string>((tv.getSupportedKeys?.() ?? []).map((k: { name: string }) => k.name));
    const names = supported.size ? TIZEN_KEYS.filter((n) => supported.has(n)) : TIZEN_KEYS;
    tv.registerKeyBatch(names);
  } catch {
    /* ignore: unsupported key on older firmware */
  }
}

const handlers = new Set<RemoteKeyHandler>();
let installed = false;

function isTextTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  const tag = el?.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || !!el?.isContentEditable;
}

/** Dispatch a raw keyboard event through the normaliser. Exported for tests. */
export function dispatchKey(ev: KeyboardEvent): void {
  const n = normalizeKey(ev);
  if (!n) return;
  // Don't hijack typing: digits and Backspace belong to text fields.
  if (isTextTarget(ev.target) && (n.key === "digit" || ev.key === "Backspace")) return;
  const out: RemoteKeyEvent = { key: n.key, digit: n.digit, original: ev };
  let handled = false;
  handlers.forEach((h) => {
    if (h(out) === true) handled = true;
  });
  if (handled) {
    ev.preventDefault();
    ev.stopPropagation();
  }
}

function install(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;
  if (platform === "tizen") registerTizenKeys();
  window.addEventListener("keydown", dispatchKey, true);
}

/** Subscribe to normalised remote keys. Returns an unsubscribe function. */
export function onRemoteKey(handler: RemoteKeyHandler): () => void {
  handlers.add(handler);
  install();
  return () => {
    handlers.delete(handler);
  };
}

/** Exit/close the app (used for Back on the root screen). Browser: no-op. */
export function exitApp(g: any = globalThis, p = platform): void {
  try {
    if (p === "webos") {
      if (typeof g.webOS?.platformBack === "function") g.webOS.platformBack();
      else if (typeof g.PalmSystem?.platformBack === "function") g.PalmSystem.platformBack();
      else g.close?.();
    } else if (p === "tizen") {
      g.tizen?.application?.getCurrentApplication?.().exit();
    }
  } catch {
    /* ignore */
  }
}

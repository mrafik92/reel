import { createSignal } from "solid-js";
import { TV } from "../mode";

/**
 * Google Cast Web Sender, loaded lazily (only when a cast menu opens) from gstatic.
 * Works in Chrome / Edge on desktop and Android; everywhere else the SDK reports
 * "unavailable" and the menu says so.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */
type Any = any;
declare global {
  interface Window {
    __onGCastApiAvailable?: (ok: boolean, reason?: string) => void;
    cast?: Any;
    chrome?: Any;
  }
}

const SDK_URL = "https://www.gstatic.com/cv/js/sender/v1/cast_sender.js?loadCastFramework=1";
export const DEFAULT_RECEIVER = "CC1AD845";

export type SdkStatus = "idle" | "loading" | "ready" | "unavailable";
/** Cast framework states: NO_DEVICES_AVAILABLE | NOT_CONNECTED | CONNECTING | CONNECTED */
export type CastFrameworkState = "NO_DEVICES_AVAILABLE" | "NOT_CONNECTED" | "CONNECTING" | "CONNECTED";

const [status, setStatus] = createSignal<SdkStatus>("idle");
const [castState, setCastState] = createSignal<CastFrameworkState>("NO_DEVICES_AVAILABLE");
/** Why it's unavailable, for the menu's small print. */
const [unavailableWhy, setWhy] = createSignal("");
export { status as castSdkStatus, castState, unavailableWhy as castUnavailableWhy };

let loading: Promise<boolean> | undefined;

/** Load + initialise the sender once. Resolves `false` when casting isn't possible here. */
export function loadCastSdk(): Promise<boolean> {
  if (loading) return loading;
  loading = new Promise<boolean>((resolve) => {
    const fail = (why: string) => {
      setWhy(why);
      setStatus("unavailable");
      resolve(false);
    };
    if (typeof window === "undefined" || TV) return fail("Not available on this device");
    if (!window.isSecureContext) return fail("Needs a secure (https) connection");
    if (window.cast?.framework) {
      init();
      return resolve(true);
    }
    setStatus("loading");
    const timer = setTimeout(() => fail("Couldn't load Google Cast"), 10_000);
    window.__onGCastApiAvailable = (ok) => {
      clearTimeout(timer);
      if (!ok || !window.cast?.framework) return fail("Use Google Chrome to cast");
      try {
        init();
        resolve(true);
      } catch {
        fail("Google Cast didn't start");
      }
    };
    const s = document.createElement("script");
    s.src = SDK_URL;
    s.async = true;
    s.onerror = () => {
      clearTimeout(timer);
      fail("Couldn't load Google Cast");
    };
    document.head.appendChild(s);
  });
  return loading;
}

let inited = false;
function init(): void {
  if (inited) return;
  inited = true;
  const { cast, chrome } = window as Any;
  const ctx = cast.framework.CastContext.getInstance();
  ctx.setOptions({
    receiverApplicationId: chrome.cast.media.DEFAULT_MEDIA_RECEIVER_APP_ID || DEFAULT_RECEIVER,
    autoJoinPolicy: chrome.cast.AutoJoinPolicy.ORIGIN_SCOPED,
    resumeSavedSession: true,
  });
  setCastState(ctx.getCastState());
  ctx.addEventListener(cast.framework.CastContextEventType.CAST_STATE_CHANGED, (e: Any) => setCastState(e.castState));
  setStatus("ready");
}

export function castContext(): Any {
  return (window as Any).cast.framework.CastContext.getInstance();
}

/** Chrome's device picker (needs a user gesture). Resolves the session, rejects on cancel. */
export async function requestCastSession(): Promise<Any> {
  if (!(await loadCastSdk())) throw new Error(unavailableWhy() || "Google Cast isn't available");
  const ctx = castContext();
  const cur = ctx.getCurrentSession();
  if (cur) return cur;
  try {
    // Rejects with an error code ("cancel", "timeout", "receiver_unavailable"…).
    const err = await ctx.requestSession();
    if (err) throw err;
  } catch (e) {
    throw new CastCancelled(String(e));
  }
  const s = ctx.getCurrentSession();
  if (!s) throw new CastCancelled("cancel");
  return s;
}

export class CastCancelled extends Error {}

export interface CastLoad {
  url: string;
  contentType: string;
  /** HLS with fMP4 segments (our transcoder) — the receiver must be told. */
  fmp4: boolean;
  live: boolean;
  start: number;
  title: string;
  subtitle?: string;
  images: string[];
  tracks: { id: number; url: string; name: string; language?: string | null }[];
  activeTrack: number | null;
}

/** Load media on the current receiver session. */
export async function loadOnReceiver(session: Any, m: CastLoad): Promise<void> {
  const { chrome } = window as Any;
  const media = chrome.cast.media;
  const info = new media.MediaInfo(m.url, m.contentType);
  info.streamType = m.live ? media.StreamType.LIVE : media.StreamType.BUFFERED;
  if (m.fmp4 && /mpegurl/i.test(m.contentType)) {
    if (media.HlsSegmentFormat) info.hlsSegmentFormat = media.HlsSegmentFormat.FMP4;
    if (media.HlsVideoSegmentFormat) info.hlsVideoSegmentFormat = media.HlsVideoSegmentFormat.FMP4;
  }
  const meta = new media.GenericMediaMetadata();
  meta.title = m.title;
  if (m.subtitle) meta.subtitle = m.subtitle;
  meta.images = m.images.map((u) => new chrome.cast.Image(u));
  info.metadata = meta;
  info.tracks = m.tracks.map((t) => {
    const tr = new media.Track(t.id, media.TrackType.TEXT);
    tr.trackContentId = t.url;
    tr.trackContentType = "text/vtt";
    tr.subtype = media.TextTrackType.SUBTITLES;
    tr.name = t.name;
    if (t.language) tr.language = t.language;
    return tr;
  });
  // Same look as our own overlay: white, bold-ish, soft shadow, faint backing.
  const style = new media.TextTrackStyle();
  style.foregroundColor = "#FFFFFFFF";
  style.backgroundColor = "#00000066";
  style.edgeType = media.TextTrackEdgeType.DROP_SHADOW;
  style.edgeColor = "#000000CC";
  style.fontScale = 1.1;
  style.fontFamily = "Arial";
  style.fontGenericFamily = media.TextTrackFontGenericFamily.SANS_SERIF;
  info.textTrackStyle = style;
  const req = new media.LoadRequest(info);
  req.autoplay = true;
  if (!m.live && m.start > 0) req.currentTime = m.start;
  if (m.activeTrack != null) req.activeTrackIds = [m.activeTrack];
  let err: unknown;
  try {
    err = await session.loadMedia(req);
  } catch (e) {
    err = e || "load_failed";
  }
  if (err) throw new Error(`The TV couldn't play this (${String((err as { code?: string }).code ?? err)})`);
}

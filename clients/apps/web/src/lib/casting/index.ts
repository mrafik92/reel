import { createSignal, type Accessor } from "solid-js";
import { api } from "../../api";
import { casting, type CastDevice, type CastPlayBody } from "../../api/casting";
import type { Selected } from "../../api/prefs";
import { castContext, loadOnReceiver, requestCastSession } from "./chromecast";

/**
 * One cast at a time, app-wide. The player (and Live TV) hand in a `CastSource` — how to
 * open a receiver-reachable stream and what the server should push to a DLNA TV — and get
 * back a `Remote` with the receiver's state mirrored into signals. The active cast outlives
 * the page that started it: going back to the same title re-attaches the remote view, and
 * keep-alive pings carry on in the background.
 */

export type TargetKind = "airplay" | "chromecast" | "dlna";
export interface CastTarget {
  kind: TargetKind;
  /** DLNA device id; "airplay" / "chromecast" otherwise. */
  id: string;
  name: string;
  device?: CastDevice;
}

export interface CastTrack {
  index: number;
  title: string;
  detail?: string;
  language?: string | null;
  /** Absolute WebVTT URL (Chromecast side-loads it); absent = burned in / not side-loadable. */
  url?: string | null;
  forced?: boolean;
}

export interface CastOpen {
  audio: number | null;
  /** null = automatic (server preferences), -1 = off */
  subtitle: number | null;
  start: number;
}

export interface CastStream {
  /** Playback session to keep alive (Chromecast / AirPlay). */
  sessionId: string;
  url: string;
  contentType: string;
  fmp4: boolean;
  live: boolean;
  audio: CastTrack[];
  subtitles: CastTrack[];
  selected?: Selected;
  /** Default: `POST /playback/{id}/progress`. Live TV: the receiver's segment requests suffice. */
  keepAlive?: (positionS: number, paused: boolean) => void;
  /** Default: `DELETE /playback/{id}`. */
  release?: () => void;
}

/** Keep a receiver's session alive / end it (library + VOD defaults, live overrides). */
function ping(s: CastStream, position: number, paused: boolean): void {
  if (s.keepAlive) s.keepAlive(position, paused);
  else api.progress(s.sessionId, { position_s: position, paused }).catch(() => undefined);
}
function release(s: CastStream): void {
  if (s.release) s.release();
  else api.stopPlayback(s.sessionId).catch(() => undefined);
}

export interface CastMeta {
  /** Identifies the title ("item:161", "vod:movie:42", "channel:7") to re-attach the remote view. */
  key: string;
  title: string;
  eyebrow?: string;
  poster?: string | null;
  backdrop?: string | null;
  live?: boolean;
  duration?: number;
}

export interface CastSource {
  meta: CastMeta;
  /** Chromecast / AirPlay: start a session whose absolute URLs the receiver can fetch. */
  open(target: "chromecast" | "airplay", o: CastOpen): Promise<CastStream>;
  /** DLNA: what the server should push to the TV (omit when DLNA can't play this). */
  dlna?: (o: CastOpen) => CastPlayBody;
  /** Track lists for the remote's pickers when no stream is fetched by us (DLNA). */
  audio?: CastTrack[];
  subtitles?: CastTrack[];
}

export type RemoteState = "connecting" | "buffering" | "playing" | "paused" | "idle";
export type EndReason = "stopped" | "ended" | "disconnected" | "error";

export interface Remote {
  kind: TargetKind;
  device: Accessor<string>;
  state: Accessor<RemoteState>;
  position: Accessor<number>;
  duration: Accessor<number>;
  /** 0..1; null = the receiver doesn't report it. */
  volume: Accessor<number | null>;
  /** The receiver's volume can be changed from here. */
  canVolume: boolean;
  stream: Accessor<CastStream | undefined>;
  play(): void;
  pause(): void;
  seek(s: number): void;
  setVolume(v: number): void;
  /** Switch subtitles in place when the receiver can (Chromecast side-loaded VTT). */
  setSubtitle?(index: number): boolean;
  /** (Re)load the title — new audio track, burned-in subtitles, or after an error. */
  load(o: CastOpen): Promise<void>;
  stop(): Promise<void>;
  /** Called once the receiver session is gone. */
  onEnd(fn: (r: EndReason, position: number) => void): void;
  dispose(): void;
}

export interface ActiveCast {
  target: CastTarget;
  source: CastSource;
  remote: Remote;
  audio: Accessor<number | null>;
  sub: Accessor<number>;
  error: Accessor<string>;
  busy: Accessor<boolean>;
}

const [active, setActive] = createSignal<ActiveCast>();
export { active as activeCast };

/** Active cast for this title, if any. */
export function castFor(key: string): ActiveCast | undefined {
  const a = active();
  return a && a.source.meta.key === key ? a : undefined;
}

const endListeners = new Set<(a: ActiveCast, r: EndReason, position: number) => void>();
/** The cast ended (stopped here or on the TV): the page resumes local playback at `position`. */
export function onCastEnd(fn: (a: ActiveCast, r: EndReason, position: number) => void): () => void {
  endListeners.add(fn);
  return () => endListeners.delete(fn);
}

let keepAlive: ReturnType<typeof setInterval> | undefined;

/**
 * Start casting. Call from the click handler: Chromecast's device picker and AirPlay's need
 * a user gesture. Rejects with `CastCancelled` when the picker is dismissed.
 */
export async function startCast(
  target: CastTarget,
  source: CastSource,
  o: CastOpen,
  airplay?: { video: HTMLVideoElement; load: (o: CastOpen) => Promise<void>; stop: () => void },
): Promise<ActiveCast> {
  const prev = active();
  if (prev && (prev.target.kind !== target.kind || prev.target.id !== target.id || prev.source.meta.key !== source.meta.key)) {
    await stopCast().catch(() => undefined);
  }
  let remote: Remote;
  if (target.kind === "chromecast") remote = await chromecastRemote(source, o);
  else if (target.kind === "dlna") remote = await dlnaRemote(target, source, o);
  else remote = airplayRemote(airplay!.video, airplay!.load, airplay!.stop);

  const [audio, setAudio] = createSignal<number | null>(o.audio);
  const [sub, setSub] = createSignal<number>(o.subtitle ?? -1);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  // Adopt the server's automatic choice.
  const sel = remote.stream()?.selected;
  if (sel) {
    if (o.audio == null && sel.audio_index != null) setAudio(sel.audio_index);
    if (o.subtitle == null) setSub(sel.subtitle_index ?? -1);
  }
  const a: ActiveCast = { target, source, remote, audio, sub, error, busy };
  tracks.set(a, { setAudio, setSub, setError, setBusy });
  setActive(a);

  remote.onEnd((r, position) => {
    if (active() !== a) return;
    finish(a, r, position);
  });
  if (keepAlive) clearInterval(keepAlive);
  // Keep the playback session alive with the receiver's position (and credit progress).
  keepAlive = setInterval(() => {
    const s = remote.stream();
    if (!s || remote.kind === "airplay") return;
    const p = remote.position();
    if (p > 1 || source.meta.live) ping(s, p, remote.state() !== "playing");
  }, 10_000);
  return a;
}

const tracks = new WeakMap<ActiveCast, {
  setAudio: (v: number | null) => void;
  setSub: (v: number) => void;
  setError: (v: string) => void;
  setBusy: (v: boolean) => void;
}>();

function finish(a: ActiveCast, r: EndReason, position: number): void {
  if (keepAlive) clearInterval(keepAlive);
  keepAlive = undefined;
  const s = a.remote.stream();
  if (s && a.remote.kind === "chromecast") {
    if (position > 1 && !a.source.meta.live) ping(s, position, true);
    release(s);
  }
  a.remote.dispose();
  setActive(undefined);
  endListeners.forEach((f) => f(a, r, position));
}

/** Stop the TV and hand back the position it reached. */
export async function stopCast(): Promise<number> {
  const a = active();
  if (!a) return 0;
  const pos = a.remote.position();
  try {
    await a.remote.stop();
  } finally {
    if (active() === a) finish(a, "stopped", pos);
  }
  return pos;
}

async function reload(a: ActiveCast, o: Partial<CastOpen>): Promise<void> {
  const t = tracks.get(a)!;
  t.setBusy(true);
  t.setError("");
  try {
    await a.remote.load({
      audio: o.audio !== undefined ? o.audio : a.audio(),
      subtitle: o.subtitle !== undefined ? o.subtitle : a.sub(),
      start: a.source.meta.live ? 0 : a.remote.position(),
    });
  } catch (e) {
    t.setError(e instanceof Error ? e.message : "The TV didn’t accept the change");
  } finally {
    t.setBusy(false);
  }
}

export function castSubtitle(index: number): void {
  const a = active();
  if (!a || index === a.sub()) return;
  tracks.get(a)!.setSub(index);
  if (a.remote.setSubtitle?.(index)) return;
  void reload(a, { subtitle: index });
}

export function castAudio(index: number): void {
  const a = active();
  if (!a || index === a.audio()) return;
  tracks.get(a)!.setAudio(index);
  void reload(a, { audio: index });
}

export function castRetry(): void {
  const a = active();
  if (a) void reload(a, {});
}

// ---------------------------------------------------------------------------------------------
// Chromecast

/* eslint-disable @typescript-eslint/no-explicit-any */
const TRACK_BASE = 1000; // Cast track ids: stream index + 1000 (keeps 0 and negatives out)

async function chromecastRemote(source: CastSource, o: CastOpen): Promise<Remote> {
  const session = await requestCastSession();
  const w = window as any;
  const fw = w.cast.framework;
  const [state, setState] = createSignal<RemoteState>("connecting");
  const [pos, setPos] = createSignal(o.start);
  const [dur, setDur] = createSignal(source.meta.duration ?? 0);
  const [vol, setVol] = createSignal<number | null>(null);
  const [stream, setStream] = createSignal<CastStream>();
  const [device] = createSignal<string>(session.getCastDevice?.()?.friendlyName || "Chromecast");
  const player = new fw.RemotePlayer();
  const ctrl = new fw.RemotePlayerController(player);
  let loaded = false;
  let ended: ((r: EndReason, p: number) => void) | undefined;
  let done = false;
  const end = (r: EndReason) => {
    if (done) return;
    done = true;
    ended?.(r, pos());
  };

  const sync = () => {
    if (!player.isConnected) {
      if (loaded) end("disconnected");
      return;
    }
    if (isFinite(player.currentTime) && player.currentTime > 0) setPos(player.currentTime);
    if (player.duration && isFinite(player.duration)) setDur(player.duration);
    setVol(player.isMuted ? 0 : typeof player.volumeLevel === "number" ? player.volumeLevel : null);
    const ps = String(player.playerState || "");
    if (ps === "PLAYING") setState("playing");
    else if (ps === "PAUSED") setState("paused");
    else if (ps === "BUFFERING" || ps === "LOADING") setState("buffering");
    else if (loaded) {
      const reason = session.getMediaSession?.()?.idleReason;
      if (reason === "FINISHED") end("ended");
      else if (reason === "ERROR") setState("idle");
    }
  };
  ctrl.addEventListener(fw.RemotePlayerEventType.ANY_CHANGE, sync);

  const load = async (oo: CastOpen) => {
    setState("connecting");
    const prev = stream();
    const s = await source.open("chromecast", oo);
    setStream(s);
    if (prev && prev.sessionId !== s.sessionId) release(prev);
    const wantSub = oo.subtitle ?? s.selected?.subtitle_index ?? -1;
    const tracks = s.subtitles.filter((t) => t.url).map((t) => ({ id: t.index + TRACK_BASE, url: t.url!, name: t.title, language: t.language }));
    await loadOnReceiver(session, {
      url: s.url,
      contentType: s.contentType,
      fmp4: s.fmp4,
      live: s.live,
      start: oo.start,
      title: source.meta.title,
      subtitle: source.meta.eyebrow,
      images: [source.meta.poster, source.meta.backdrop].filter((x): x is string => !!x).map((u) => absoluteOn(u, s.url)),
      tracks,
      activeTrack: wantSub >= 0 && tracks.some((t) => t.id === wantSub + TRACK_BASE) ? wantSub + TRACK_BASE : null,
    });
    loaded = true;
    sync();
  };

  try {
    await load(o);
  } catch (e) {
    ctrl.removeEventListener(fw.RemotePlayerEventType.ANY_CHANGE, sync);
    const s = stream();
    if (s) release(s);
    throw e;
  }

  return {
    kind: "chromecast",
    canVolume: true,
    device,
    state,
    position: pos,
    duration: dur,
    volume: vol,
    stream,
    play: () => {
      if (player.isPaused) ctrl.playOrPause();
    },
    pause: () => {
      if (!player.isPaused) ctrl.playOrPause();
    },
    seek: (t) => {
      player.currentTime = t;
      setPos(t);
      ctrl.seek();
    },
    setVolume: (v) => {
      if (player.isMuted) ctrl.muteOrUnmute();
      player.volumeLevel = Math.max(0, Math.min(1, v));
      setVol(player.volumeLevel);
      ctrl.setVolumeLevel();
    },
    setSubtitle: (index) => {
      const s = stream();
      const ms = session.getMediaSession?.();
      if (!s || !ms) return false;
      if (index >= 0 && !s.subtitles.some((t) => t.index === index && t.url)) return false;
      // Leaving burned-in subtitles needs a clean stream.
      if (s.selected && s.selected.subtitle_index != null && !s.subtitles.some((t) => t.index === s.selected!.subtitle_index && t.url)) return false;
      const req = new w.chrome.cast.media.EditTracksInfoRequest(index >= 0 ? [index + TRACK_BASE] : []);
      ms.editTracksInfo(req, () => undefined, () => undefined);
      return true;
    },
    load,
    stop: async () => {
      castContext().endCurrentSession(true);
    },
    onEnd: (fn) => (ended = fn),
    dispose: () => {
      done = true;
      ctrl.removeEventListener(fw.RemotePlayerEventType.ANY_CHANGE, sync);
    },
  };
}

/** Our image URLs are relative to the web origin; the receiver needs the LAN cast base. */
function absoluteOn(path: string, mediaUrl: string): string {
  if (/^https?:/i.test(path)) return path;
  const m = /^(https?:\/\/[^/]+)/i.exec(mediaUrl);
  return m ? m[1] + (path.charAt(0) === "/" ? path : "/" + path) : path;
}

// ---------------------------------------------------------------------------------------------
// DLNA (server-driven)

async function dlnaRemote(target: CastTarget, source: CastSource, o: CastOpen): Promise<Remote> {
  if (!source.dlna) throw new Error("This can’t be sent to that TV");
  const [state, setState] = createSignal<RemoteState>("connecting");
  const [pos, setPos] = createSignal(o.start);
  const [dur, setDur] = createSignal(source.meta.duration ?? 0);
  const [vol, setVol] = createSignal<number | null>(null);
  const [stream] = createSignal<CastStream>();
  let id = (await casting.play(target.id, source.dlna(o))).cast_session_id;
  let startedAt = Date.now();
  let everPlayed = false;
  let ended: ((r: EndReason, p: number) => void) | undefined;
  let done = false;
  let quietUntil = 0; // ignore polls right after a command (renderers report the old state)
  let failures = 0;

  const end = (r: EndReason) => {
    if (done) return;
    done = true;
    clearInterval(timer);
    ended?.(r, pos());
  };
  const poll = async () => {
    if (done) return;
    try {
      const s = await casting.session(id);
      failures = 0;
      if (done || Date.now() < quietUntil) return;
      if (s.duration_s && s.duration_s > 0) setDur(s.duration_s);
      if (s.position_s > 0 || everPlayed) setPos(s.position_s);
      if (typeof s.volume === "number") setVol(s.volume / 100);
      if (s.state === "playing") everPlayed = true;
      if (s.state === "stopped") {
        // TVs report STOPPED while they switch streams: give a fresh start some time.
        if (!everPlayed && Date.now() - startedAt < 20_000) return setState("connecting");
        const nearEnd = dur() > 0 && pos() > dur() - 30;
        end(nearEnd ? "ended" : "stopped");
        return;
      }
      setState(s.state);
    } catch {
      if (++failures >= 5) end("disconnected");
    }
  };
  const timer = setInterval(() => void poll(), 1000);
  void poll();

  const cmd = (fn: () => Promise<void>) => {
    quietUntil = Date.now() + 1500;
    fn().catch(() => undefined);
  };

  return {
    kind: "dlna",
    canVolume: target.device ? target.device.supports_volume : true,
    device: () => target.name,
    state,
    position: pos,
    duration: dur,
    volume: vol,
    stream,
    play: () => {
      setState("playing");
      cmd(() => casting.control(id, "play"));
    },
    pause: () => {
      setState("paused");
      cmd(() => casting.control(id, "pause"));
    },
    seek: (t) => {
      setPos(t);
      cmd(() => casting.control(id, "seek", t));
    },
    setVolume: (v) => {
      const l = Math.max(0, Math.min(1, v));
      setVol(l);
      cmd(() => casting.volume(id, l * 100));
    },
    load: async (oo) => {
      setState("connecting");
      quietUntil = Date.now() + 2500;
      id = (await casting.play(target.id, source.dlna!(oo))).cast_session_id;
      startedAt = Date.now();
      everPlayed = false;
    },
    stop: async () => {
      clearInterval(timer);
      await casting.control(id, "stop").catch(() => undefined);
    },
    onEnd: (fn) => (ended = fn),
    dispose: () => {
      done = true;
      clearInterval(timer);
    },
  };
}

// ---------------------------------------------------------------------------------------------
// AirPlay: Safari routes the page's own <video> to the receiver; the remote drives that element.

function airplayRemote(video: HTMLVideoElement, load: (o: CastOpen) => Promise<void>, stop: () => void): Remote {
  const [state, setState] = createSignal<RemoteState>(video.paused ? "paused" : "playing");
  const [pos, setPos] = createSignal(video.currentTime);
  const [dur, setDur] = createSignal(isFinite(video.duration) ? video.duration : 0);
  const [vol, setVol] = createSignal<number | null>(video.volume);
  const [stream] = createSignal<CastStream>();
  let ended: ((r: EndReason, p: number) => void) | undefined;
  const on: [string, () => void][] = [
    ["play", () => setState("playing")],
    ["playing", () => setState("playing")],
    ["pause", () => setState("paused")],
    ["waiting", () => setState("buffering")],
    ["timeupdate", () => setPos(video.currentTime)],
    ["durationchange", () => isFinite(video.duration) && setDur(video.duration)],
    ["volumechange", () => setVol(video.muted ? 0 : video.volume)],
    ["ended", () => ended?.("ended", video.currentTime)],
    [
      "webkitcurrentplaybacktargetiswirelesschanged",
      () => {
        if (!(video as unknown as { webkitCurrentPlaybackTargetIsWireless?: boolean }).webkitCurrentPlaybackTargetIsWireless) {
          ended?.("disconnected", video.currentTime);
        }
      },
    ],
  ];
  on.forEach(([e, f]) => video.addEventListener(e, f));
  return {
    kind: "airplay",
    canVolume: true,
    device: () => "AirPlay",
    state,
    position: pos,
    duration: dur,
    volume: vol,
    stream,
    play: () => void video.play().catch(() => undefined),
    pause: () => video.pause(),
    seek: (t) => {
      video.currentTime = t;
      setPos(t);
    },
    setVolume: (v) => {
      video.muted = false;
      video.volume = Math.max(0, Math.min(1, v));
    },
    load,
    stop: async () => stop(),
    onEnd: (fn) => (ended = fn),
    dispose: () => on.forEach(([e, f]) => video.removeEventListener(e, f)),
  };
}

/** Safari exposes AirPlay on media elements. */
export const AIRPLAY_SUPPORTED =
  typeof window !== "undefined" && "WebKitPlaybackTargetAvailabilityEvent" in window;

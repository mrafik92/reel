/// <reference path="./platform/webos.d.ts" />
/// <reference path="./platform/tizen.d.ts" />
import { parseVtt, type VttCue } from "./platform/vtt";
import { RecoveryPolicy, failureKind, hlsConfig, recoveryReason, type FailureKind, type RecoveryAction } from "./recovery";
import { abrConfig, fragmentKbps, planLevels, variantIdOf, type VariantOptions, type VariantSelect } from "./abr";

/** Sidecar text track (WebVTT) to attach to the stream. */
export interface ExternalTextTrack {
  id?: string;
  url: string;
  label?: string;
  language?: string;
}

export interface LoadOptions {
  startPositionSec?: number;
  autoplay?: boolean;
  headers?: Record<string, string>;
  /** Sidecar subtitles (WebVTT). Native adapters attach them; others may ignore. */
  textTracks?: ExternalTextTrack[];
  /** A live stream: stay near the edge, and recover to the edge instead of the playhead. */
  live?: boolean;
  /**
   * A multi-variant HLS session (#111): start variant, estimate seed and pin/cap. hls.js only;
   * other adapters and native HLS ignore it (they get single-variant sessions, Q6).
   */
  variants?: VariantOptions;
}

export interface PlayerTrack {
  id: string;
  label: string;
  language?: string;
  selected: boolean;
}
export interface PlayerTracks {
  audio: PlayerTrack[];
  text: PlayerTrack[];
}

export interface PlayerEventMap {
  timeupdate: { positionSec: number; durationSec: number };
  play: undefined;
  pause: undefined;
  ended: undefined;
  buffering: boolean;
  /** Fired once metadata is known. */
  ready: { durationSec: number };
  /** Furthest buffered position ahead of the playhead. */
  buffered: { endSec: number };
  error: { message: string; fatal: boolean };
  /** Audio/text track lists changed (native adapters only). */
  tracks: PlayerTracks;
  /** Current subtitle cue rendered by the app (AVPlay); `null` text clears it. */
  cue: { text: string | null };
  /** The adapter lost the stream and is getting it back by itself; `null` once it plays again. */
  recovering: { attempt: number; reason: string } | null;
  /**
   * The variant being played changed (multi-variant HLS, #111). `id` is the variant of the level
   * hls.js switched to; `ids` are the variants it can play, ascending bandwidth.
   */
  variant: { id: string | undefined; ids: string[] };
  /** A media download's measured throughput (hls.js fragments), for the app's bandwidth estimate. */
  throughput: { kbps: number };
}

export type PlayerEventHandler<K extends keyof PlayerEventMap> = (data: PlayerEventMap[K]) => void;

export interface PlayerAdapter {
  load(url: string, opts?: LoadOptions): Promise<void>;
  play(): Promise<void>;
  pause(): void;
  seek(positionSec: number): void;
  destroy(): void;
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void;

  // ---- optional (additive) capabilities; present on native adapters ----
  getTracks?(): PlayerTracks;
  selectAudioTrack?(id: string): void;
  /** `null` turns subtitles off. */
  selectTextTrack?(id: string | null): void;
  /**
   * Multi-variant HLS (#111): pin a variant or limit ABR to some, without reloading. Returns
   * `false` when the current stream can't do it (single variant, the variant isn't there): the
   * app re-decides instead.
   */
  selectVariant?(sel: VariantSelect): boolean;
}

export class Emitter {
  private m = new Map<string, Set<(d: any) => void>>();
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void {
    let s = this.m.get(type);
    if (!s) this.m.set(type, (s = new Set()));
    s.add(h);
    return () => s!.delete(h);
  }
  emit<K extends keyof PlayerEventMap>(type: K, data: PlayerEventMap[K]): void {
    this.m.get(type)?.forEach((h) => h(data));
  }
  clear(): void {
    this.m.clear();
  }
}

/** Standard <video> element → PlayerEventMap wiring (shared by HlsJs and WebOs adapters).
 *  `onMediaError` may take over element errors (return `true` = handled, don't emit). */
function wireVideoEvents(v: HTMLVideoElement, ev: Emitter, onMediaError?: () => boolean): void {
  v.addEventListener("timeupdate", () =>
    ev.emit("timeupdate", { positionSec: v.currentTime, durationSec: v.duration || 0 }));
  v.addEventListener("play", () => ev.emit("play", undefined));
  v.addEventListener("pause", () => ev.emit("pause", undefined));
  v.addEventListener("ended", () => ev.emit("ended", undefined));
  v.addEventListener("waiting", () => ev.emit("buffering", true));
  v.addEventListener("playing", () => ev.emit("buffering", false));
  v.addEventListener("loadedmetadata", () => ev.emit("ready", { durationSec: v.duration || 0 }));
  v.addEventListener("progress", () => {
    const b = v.buffered;
    let end = 0;
    for (let i = 0; i < b.length; i++) {
      if (b.start(i) <= v.currentTime + 0.5) end = Math.max(end, b.end(i));
    }
    ev.emit("buffered", { endSec: end });
  });
  v.addEventListener("error", () => {
    if (onMediaError?.()) return;
    ev.emit("error", { message: v.error?.message ?? "media error", fatal: true });
  });
}

/**
 * Play HLS with the browser's own player instead of hls.js? Only on Apple browsers (Safari's
 * player is the reference one, and AirPlay needs it) and where MSE is missing. Chrome ≥ 14x also
 * answers `canPlayType("application/vnd.apple.mpegurl")` now, but its built-in player has a small
 * fixed buffer and gives up on the first bad segment (`CHUNK_DEMUXER_ERROR_APPEND_FAILED`, #91), so
 * there hls.js stays in charge of buffering and recovery.
 */
export function prefersNativeHls(v: HTMLVideoElement, nav: { vendor?: string; userAgent?: string } | undefined = typeof navigator !== "undefined" ? navigator : undefined): boolean {
  if (v.canPlayType("application/vnd.apple.mpegurl") === "") return false;
  const g = globalThis as { MediaSource?: unknown; ManagedMediaSource?: unknown };
  if (!g.MediaSource && !g.ManagedMediaSource) return true;
  const apple = /Apple/i.test(nav?.vendor ?? "") && !/Chrome|Chromium|CriOS|Edg|Android/i.test(nav?.userAgent ?? "");
  return apple;
}

/** A stall this long (buffering while meant to play) counts as a lost stream. */
const STALL_VOD_MS = 25_000;
const STALL_LIVE_MS = 12_000;
/** Uninterrupted playback this long ends a recovery streak. */
const HEALTHY_MS = 20_000;

/**
 * Desktop/mobile browsers: hls.js on MSE, or the element itself (direct play, native HLS on
 * Safari). Fatal errors and long stalls are recovered by the adapter (see `recovery.ts`): reload at
 * the playhead (VOD) or at the live edge (live), and only after repeated failures is a fatal `error`
 * emitted. While it works on it, `recovering` carries the attempt; `null` once the picture moves again.
 */
export class HlsJsAdapter implements PlayerAdapter {
  private hls?: import("hls.js").default;
  private HlsCtor?: typeof import("hls.js").default;
  private ev = new Emitter();
  private url = "";
  private opts: LoadOptions = {};
  private live = false;
  private policy: RecoveryPolicy;
  private pending?: ReturnType<typeof setTimeout>;
  private recovering = false;
  private lastActionAt = 0;
  private stallTimer?: ReturnType<typeof setTimeout>;
  private healthyFrom = 0;
  private lastPos = 0;
  private destroyed = false;
  /** Position to resume from on a rebuild (the element may already be reset). */
  private resumeAt = 0;
  /**
   * The viewer wants it playing (play()/autoplay, or the element's own controls/media keys).
   * Recovery restores this state and never starts a paused video by itself (#100): a reload or
   * a rebuild resets the element, so `video.paused` can't tell afterwards.
   */
  private wantPlay = false;
  /** The segment hls.js is fetching and since when; when the last one arrived (stall diagnostics). */
  private fragLoading?: { sn: number | string; at: number };
  private fragLoadedAt = 0;
  /** A playlist request in flight, and when the current pipeline was built (stall diagnostics). */
  private playlistLoading?: { what: string; at: number };
  private attachedAt = 0;
  /** Variant ids of the current `hls.levels` and the one playing (multi-variant sessions). */
  private levelIds: (string | undefined)[] = [];
  private levelId?: string;

  /** `maxAttempts`: recovery attempts before a fatal `error` (default 8). */
  constructor(private video: HTMLVideoElement, o: { maxAttempts?: number } = {}) {
    this.policy = new RecoveryPolicy({ maxAttempts: o.maxAttempts });
    wireVideoEvents(video, this.ev, () => this.onElementError());
    video.addEventListener("waiting", () => this.armStall());
    video.addEventListener("stalled", () => this.armStall());
    video.addEventListener("playing", () => this.clearStall());
    video.addEventListener("pause", () => this.clearStall());
    video.addEventListener("timeupdate", () => this.onProgress());
    // Play/pause from outside the app (media keys, PiP window): follow it, except for the
    // pause/play a recovery step causes itself.
    video.addEventListener("play", () => {
      if (!this.inRecovery()) this.wantPlay = true;
    });
    video.addEventListener("pause", () => {
      if (!this.inRecovery() && !video.error && !video.ended) this.wantPlay = false;
    });
    video.addEventListener("canplay", () => this.loadedWhilePaused());
    video.addEventListener("loadeddata", () => this.loadedWhilePaused());
  }

  /** Recovered while paused: the stream is back once data arrives (the picture won't move). */
  private loadedWhilePaused(): void {
    if (this.recovering && !this.pending && !this.wantPlay) this.setRecovering(null);
  }

  /** A recovery step is scheduled or running (the element events it causes aren't the viewer's). */
  private inRecovery(): boolean {
    return this.recovering || !!this.pending;
  }

  /** Back to the state the viewer wanted: play only if it was playing (#100). */
  private resume(): void {
    if (this.destroyed || !this.wantPlay) return;
    this.video.play().catch(() => undefined);
    this.armStall();
  }

  /** Whether the viewer wants it playing, and whether a recovery is under way (tests, diagnostics). */
  get state(): { wantPlay: boolean; recovering: boolean } {
    return { wantPlay: this.wantPlay, recovering: this.recovering };
  }

  async load(url: string, opts: LoadOptions = {}): Promise<void> {
    this.cancelPending();
    this.clearStall();
    this.policy.healthy();
    this.setRecovering(null);
    this.url = url;
    this.opts = opts;
    this.live = !!opts.live;
    this.resumeAt = opts.startPositionSec ?? 0;
    this.wantPlay = !!opts.autoplay;
    this.levelId = undefined;
    // Own copy: `selectVariant` updates the selection a rebuild starts from.
    if (opts.variants) this.opts = { ...opts, variants: { ...opts.variants } };
    await this.attach(opts.startPositionSec);
    if (opts.autoplay) await this.play();
  }

  /** (Re)build the pipeline for `this.url` at `start` (undefined/-1 = default / live edge). */
  private async attach(start: number | undefined): Promise<void> {
    this.hls?.destroy();
    this.hls = undefined;
    // Diagnostics describe this pipeline only: a rebuild starts with nothing loaded.
    this.fragLoading = undefined;
    this.fragLoadedAt = 0;
    this.playlistLoading = undefined;
    this.attachedAt = Date.now();
    const v = this.video;
    const url = this.url;
    const isHls = /\.m3u8(\?|$)/.test(url);
    const nativeHls = isHls && prefersNativeHls(v);
    // hls.js is only fetched for HLS on MSE browsers; direct play stays lean.
    const Hls = isHls && !nativeHls ? (this.HlsCtor ??= (await import("hls.js")).default) : undefined;
    if (this.destroyed || url !== this.url) return;
    if (Hls && Hls.isSupported()) {
      const headers = this.opts.headers;
      const variants = !this.live ? this.opts.variants : undefined;
      this.levelIds = [];
      const hls = new Hls({
        ...(hlsConfig({ live: this.live, startPosition: start && start > 0 ? start : -1 }) as Partial<import("hls.js").HlsConfig>),
        ...(variants ? (abrConfig(variants) as Partial<import("hls.js").HlsConfig>) : {}),
        xhrSetup: headers
          ? (xhr: XMLHttpRequest) => Object.entries(headers).forEach(([k, val]) => xhr.setRequestHeader(k, val))
          : undefined,
      });
      this.hls = hls;
      hls.on(Hls.Events.ERROR, (_e, d) => {
        if (hls !== this.hls) return;
        if (!d.fatal) {
          this.ev.emit("error", { message: d.details, fatal: false });
          return;
        }
        this.fail(failureKind(d.type), d.details);
      });
      hls.on(Hls.Events.LEVEL_LOADED, (_e, d) => {
        if (hls === this.hls && d.details?.live) this.live = true;
      });
      hls.on(Hls.Events.FRAG_BUFFERED, () => {
        if (hls === this.hls) this.loadedWhilePaused();
      });
      hls.on(Hls.Events.FRAG_LOADING, (_e, d) => {
        if (hls === this.hls) this.fragLoading = { sn: d.frag.sn, at: Date.now() };
      });
      hls.on(Hls.Events.FRAG_LOADED, (_e, d) => {
        if (hls !== this.hls) return;
        this.fragLoading = undefined;
        this.fragLoadedAt = Date.now();
        const kbps = d.frag.type === "main" ? fragmentKbps(d.frag.stats) : undefined;
        if (kbps) this.ev.emit("throughput", { kbps });
      });
      if (variants) {
        hls.on(Hls.Events.MANIFEST_PARSED, () => {
          if (hls !== this.hls) return;
          this.levelIds = hls.levels.map((l) => variantIdOf(l.uri));
          // A rebuild (recovery) resumes on the variant that was playing, not the first one.
          const plan = planLevels(this.levelIds, { start: this.levelId ?? variants.start, select: variants.select })
            ?? planLevels(this.levelIds, { start: this.levelId ?? variants.start });
          if (plan) {
            hls.autoLevelCapping = plan.cap;
            hls.startLevel = plan.start;
            if (plan.pin >= 0) hls.loadLevel = plan.pin;
          }
          hls.startLoad(hls.config.startPosition);
        });
        hls.on(Hls.Events.LEVEL_SWITCHED, (_e, d) => {
          if (hls !== this.hls) return;
          this.levelId = this.levelIds[d.level];
          this.ev.emit("variant", { id: this.levelId, ids: this.levelIds.filter((x): x is string => !!x) });
        });
      }
      const playlist = (what: string | undefined) => () => {
        if (hls === this.hls) this.playlistLoading = what ? { what, at: Date.now() } : undefined;
      };
      hls.on(Hls.Events.MANIFEST_LOADING, playlist("the playlist"));
      hls.on(Hls.Events.MANIFEST_LOADED, playlist(undefined));
      hls.on(Hls.Events.LEVEL_LOADING, playlist("the media playlist"));
      hls.on(Hls.Events.LEVEL_LOADED, playlist(undefined));
      hls.loadSource(url);
      hls.attachMedia(v);
    } else {
      v.src = url; // direct play or native HLS (Safari, TV browsers)
      if (start && start > 0) {
        const seek = () => {
          v.removeEventListener("loadedmetadata", seek);
          v.currentTime = start;
        };
        v.addEventListener("loadedmetadata", seek);
      }
      v.load();
    }
  }

  private position(): number {
    const t = this.video.currentTime;
    if (t > 0) this.resumeAt = t;
    return this.resumeAt;
  }

  /** `<video>` error: with hls.js attached it's an MSE/decoder failure we can recover from. */
  private onElementError(): boolean {
    if (this.destroyed || !this.url) return false;
    // hls.js already reported this one (bufferAppendError & co.) a moment ago.
    if (this.pending || Date.now() - this.lastActionAt < 1000) return true;
    return this.fail("media", this.video.error?.message || "media error");
  }

  /** A fatal failure: schedule the next recovery step, or give up (emitting the fatal `error`).
   *  Returns `true` when handled here — the caller must not report it again. */
  private fail(kind: FailureKind, details: string): boolean {
    if (this.destroyed) return false;
    if (this.pending) return true; // one recovery at a time
    this.clearStall();
    this.healthyFrom = 0;
    const step = this.policy.next(kind, { live: this.live, position: this.position() });
    if (step.action.kind === "giveUp") {
      this.setRecovering(null);
      this.ev.emit("error", { message: details, fatal: true });
      return true;
    }
    this.ev.emit("error", { message: details, fatal: false });
    this.setRecovering({ attempt: step.attempt, reason: recoveryReason(kind, this.live) });
    const run = () => {
      this.pending = undefined;
      this.lastActionAt = Date.now();
      this.run(step.action);
    };
    if (step.delayMs > 0) this.pending = setTimeout(run, step.delayMs);
    else run();
    return true;
  }

  private run(a: RecoveryAction): void {
    if (this.destroyed) return;
    const hls = this.hls;
    const v = this.video;
    if (a.kind === "startLoad" && hls) {
      hls.startLoad(a.position);
      // Live: we fell out of the window — go to where the edge is now.
      const edge = (hls as unknown as { liveSyncPosition?: number | null }).liveSyncPosition;
      if (this.live && edge != null && isFinite(edge) && v.currentTime < edge - 1) v.currentTime = edge;
      this.resume();
      return;
    }
    if (a.kind === "recoverMedia" && hls) {
      if (a.swapAudio) hls.swapAudioCodec();
      hls.recoverMediaError();
      this.resume();
      return;
    }
    // reload (or no hls.js instance to nudge: direct play / native HLS)
    const at = a.kind === "reload" || a.kind === "startLoad" ? a.position : this.position();
    void this.attach(at > 0 ? at : undefined).then(() => this.resume());
  }

  private setRecovering(r: { attempt: number; reason: string } | null): void {
    if (!r && !this.recovering) return;
    this.recovering = !!r;
    this.ev.emit("recovering", r);
  }

  /** Waiting for data while meant to play: if it lasts, treat it like a network failure. */
  private armStall(): void {
    if (this.stallTimer || this.destroyed || !this.url) return;
    this.stallTimer = setTimeout(() => {
      this.stallTimer = undefined;
      const v = this.video;
      if (this.destroyed || v.paused || v.ended) return;
      // Still no data ahead of the playhead?
      if (v.readyState >= 3) return;
      const why = this.stallDetails();
      console.warn(`[player] ${why}`);
      this.fail("network", why);
    }, this.live ? STALL_LIVE_MS : STALL_VOD_MS);
  }
  /**
   * Why playback stalled, for the console and the error card (#96): a segment request that never
   * finishes (a wedged connection, e.g. HTTP/3 over Tailscale) reads differently from a player that
   * stopped asking for segments.
   */
  stallDetails(now = Date.now()): string {
    const s = (ms: number) => `${Math.round(ms / 1000)} s`;
    const base = `stalled at ${this.video.currentTime.toFixed(1)} s`;
    if (this.fragLoading) return `${base}: segment ${this.fragLoading.sn} requested ${s(now - this.fragLoading.at)} ago and still loading`;
    if (this.playlistLoading) return `${base}: ${this.playlistLoading.what} requested ${s(now - this.playlistLoading.at)} ago and still loading`;
    if (this.fragLoadedAt) return `${base}: no segment requested since one arrived ${s(now - this.fragLoadedAt)} ago`;
    if (this.attachedAt) return `${base}: no segment arrived since the player was (re)started ${s(now - this.attachedAt)} ago`;
    return `${base}: no data`;
  }
  private clearStall(): void {
    if (this.stallTimer) clearTimeout(this.stallTimer);
    this.stallTimer = undefined;
  }
  private cancelPending(): void {
    if (this.pending) clearTimeout(this.pending);
    this.pending = undefined;
  }

  /** The picture moves again → recovered; a long healthy stretch resets the attempt budget. */
  private onProgress(): void {
    const v = this.video;
    const t = v.currentTime;
    const moved = Math.abs(t - this.lastPos) > 0.01 && !v.paused;
    this.lastPos = t;
    if (!moved) return;
    if (t > 0) this.resumeAt = t;
    this.clearStall();
    const now = Date.now();
    if (this.recovering) {
      this.setRecovering(null);
      this.healthyFrom = now;
    }
    if (!this.healthyFrom) this.healthyFrom = now;
    if (this.policy.used && now - this.healthyFrom > HEALTHY_MS) this.policy.healthy();
  }

  /**
   * Pin a variant or cap ABR, on the stream that's playing. A pin switches at the next fragment
   * boundary (the buffer ahead is replaced); a cap below the playing variant does the same, and
   * anything else lets ABR carry on from what's buffered.
   */
  selectVariant(sel: VariantSelect): boolean {
    const hls = this.hls;
    const v = this.opts.variants;
    if (!hls || !v || this.live) return false;
    const plan = planLevels(this.levelIds, { select: sel });
    if (!plan) return false;
    v.select = sel;
    hls.autoLevelCapping = plan.cap;
    if (plan.pin >= 0) {
      // Already loading that level (ABR picked it): just stop switching, keep the buffer.
      if (hls.loadLevel === plan.pin) hls.loadLevel = plan.pin;
      else hls.nextLevel = plan.pin;
    } else if (plan.cap >= 0 && hls.loadLevel > plan.cap) {
      hls.nextLevel = -1; // ABR again, and drop what's buffered above the cap
    } else if (!hls.autoLevelEnabled) {
      hls.loadLevel = -1; // ABR again, keeping the buffer
    }
    return true;
  }

  play(): Promise<void> {
    this.wantPlay = true;
    return this.video.play();
  }
  pause(): void {
    this.wantPlay = false;
    this.video.pause();
  }
  seek(positionSec: number): void {
    this.resumeAt = positionSec;
    this.video.currentTime = positionSec;
  }
  destroy(): void {
    this.destroyed = true;
    this.cancelPending();
    this.clearStall();
    this.hls?.destroy();
    this.hls = undefined;
    this.video.removeAttribute("src");
    this.video.load();
    this.ev.clear();
  }
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void {
    return this.ev.on(type, h);
  }
}

function mimeFor(url: string): string | undefined {
  const path = url.split(/[?#]/)[0] ?? "";
  if (/\.m3u8$/i.test(path)) return "application/vnd.apple.mpegurl";
  if (/\.(ts|m2ts)$/i.test(path)) return "video/mp2t";
  if (/\.mkv$/i.test(path)) return "video/x-matroska";
  if (/\.(mp4|m4v)$/i.test(path)) return "video/mp4";
  return undefined;
}

/**
 * LG webOS: plain HTML5 <video> on the native pipeline. webOS plays HLS and MPEG-TS (IPTV passthrough)
 * natively and decodes HEVC/HDR10/DV in hardware, so no hls.js. Audio tracks via `video.audioTracks`,
 * subtitles via `<track kind="subtitles">` (WebVTT) or the embedded `video.textTracks`.
 */
export class WebOsAdapter implements PlayerAdapter {
  private ev = new Emitter();
  private sidecar: HTMLTrackElement[] = [];

  constructor(private video: HTMLVideoElement) {
    wireVideoEvents(video, this.ev);
    const emitTracks = () => this.ev.emit("tracks", this.getTracks());
    video.addEventListener("loadedmetadata", emitTracks);
    const at = (video as any).audioTracks;
    at?.addEventListener?.("addtrack", emitTracks);
    at?.addEventListener?.("change", emitTracks);
    video.textTracks?.addEventListener?.("addtrack", emitTracks);
  }

  async load(url: string, opts: LoadOptions = {}): Promise<void> {
    const v = this.video;
    this.clearSource();
    if (opts.textTracks?.length) v.crossOrigin = "anonymous"; // file:// app → server needs CORS for <track>
    const type = mimeFor(url);
    if (type) {
      const s = document.createElement("source");
      s.src = url;
      s.type = type;
      v.appendChild(s);
    } else {
      v.src = url;
    }
    opts.textTracks?.forEach((t, i) => {
      const el = document.createElement("track");
      el.kind = "subtitles";
      el.src = t.url;
      el.label = t.label ?? t.language ?? `Subtitle ${i + 1}`;
      if (t.language) el.srclang = t.language;
      el.setAttribute("data-reel-id", t.id ?? `ext:${i}`);
      v.appendChild(el);
      this.sidecar.push(el);
    });
    v.load();
    const start = opts.startPositionSec;
    if (start) {
      const seek = () => {
        v.removeEventListener("loadedmetadata", seek);
        v.currentTime = start;
      };
      v.addEventListener("loadedmetadata", seek);
    }
    if (opts.autoplay) await this.play();
  }

  private clearSource(): void {
    const v = this.video;
    v.removeAttribute("src");
    Array.from(v.querySelectorAll("source,track")).forEach((n) => n.remove());
    this.sidecar = [];
  }

  play(): Promise<void> {
    return Promise.resolve(this.video.play());
  }
  pause(): void {
    this.video.pause();
  }
  seek(positionSec: number): void {
    this.video.currentTime = positionSec;
  }

  getTracks(): PlayerTracks {
    const v = this.video;
    const audio: PlayerTrack[] = [];
    const at = (v as any).audioTracks as ArrayLike<{ id?: string; label?: string; language?: string; enabled: boolean }> | undefined;
    if (at) {
      for (let i = 0; i < at.length; i++) {
        const t = at[i]!;
        audio.push({
          id: `a:${i}`,
          label: t.label || (t.language ? t.language.toUpperCase() : `Audio ${i + 1}`),
          language: t.language || undefined,
          selected: !!t.enabled,
        });
      }
    }
    const text: PlayerTrack[] = [];
    const tt = v.textTracks;
    for (let i = 0; i < (tt?.length ?? 0); i++) {
      const t = tt[i]!;
      if (t.kind !== "subtitles" && t.kind !== "captions") continue;
      const el = this.sidecar.find((e) => e.track === t);
      text.push({
        id: el?.getAttribute("data-reel-id") ?? `t:${i}`,
        label: t.label || (t.language ? t.language.toUpperCase() : `Subtitle ${i + 1}`),
        language: t.language || undefined,
        selected: t.mode === "showing",
      });
    }
    return { audio, text };
  }

  selectAudioTrack(id: string): void {
    const at = (this.video as any).audioTracks as ArrayLike<{ enabled: boolean }> | undefined;
    if (!at) return;
    const idx = Number(id.replace(/^a:/, ""));
    for (let i = 0; i < at.length; i++) at[i]!.enabled = i === idx;
    this.ev.emit("tracks", this.getTracks());
  }

  selectTextTrack(id: string | null): void {
    const tt = this.video.textTracks;
    for (let i = 0; i < (tt?.length ?? 0); i++) {
      const t = tt[i]!;
      if (t.kind !== "subtitles" && t.kind !== "captions") continue;
      const el = this.sidecar.find((e) => e.track === t);
      const tid = el?.getAttribute("data-reel-id") ?? `t:${i}`;
      t.mode = id !== null && tid === id ? "showing" : "disabled";
    }
    this.ev.emit("tracks", this.getTracks());
  }

  destroy(): void {
    this.clearSource();
    this.video.load();
    this.ev.clear();
  }
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void {
    return this.ev.on(type, h);
  }
}

// ---------------------------------------------------------------------------------------------
// Samsung Tizen AVPlay
// ---------------------------------------------------------------------------------------------

const AVPLAY_OBJECT_ID = "reel-avplayer";
const AVPLAY_STYLE_ID = "reel-avplay-style";
const AVPLAY_ACTIVE_CLASS = "reel-avplay-active";

function parseExtra(extra: string): Record<string, string> {
  try {
    const o = JSON.parse(extra);
    return o && typeof o === "object" ? (o as Record<string, string>) : {};
  } catch {
    return {};
  }
}

function trackLabel(kind: string, n: number, extra: Record<string, string>): { label: string; language?: string } {
  const lang = extra.language || extra.track_lang || undefined;
  const l = lang && lang !== "und" && lang !== "0" ? lang : undefined;
  return { label: l ? l.toUpperCase() : `${kind} ${n + 1}`, language: l };
}

/**
 * Samsung Tizen: `webapis.avplay`. Video renders on a plane *behind* the web layer through an
 * `<object type="application/avplayer">`; while active we add `html.reel-avplay-active` and force a
 * transparent page background so it shows through (the app must not paint opaque layers over it).
 */
export class AvPlayAdapter implements PlayerAdapter {
  private ev = new Emitter();
  private obj?: HTMLObjectElement;
  private url = "";
  private durationSec = 0;
  private positionSec = 0;
  private resumeAfterRestore = false;
  private suspended = false;
  private opened = false;
  private audioSel = -1;
  private textSel: string | null = null; // "emb:N" | "ext:N" | null
  private ext: { id: string; label: string; language?: string; cues?: VttCue[] }[] = [];
  private lastCue: string | null = null;
  private onVis = () => this.onVisibility();

  /** @param video optional <video> element from the app; hidden while AVPlay is active. */
  constructor(private video?: HTMLVideoElement, private api: () => AVPlay | undefined = () => (typeof webapis !== "undefined" ? webapis?.avplay : undefined)) {
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", this.onVis);
      document.addEventListener("webkitvisibilitychange", this.onVis);
    }
  }

  private get av(): AVPlay {
    const a = this.api();
    if (!a) throw new Error("webapis.avplay is not available");
    return a;
  }

  private ensureObject(): void {
    if (typeof document === "undefined") return;
    if (!document.getElementById(AVPLAY_STYLE_ID)) {
      const st = document.createElement("style");
      st.id = AVPLAY_STYLE_ID;
      st.textContent =
        `html.${AVPLAY_ACTIVE_CLASS},html.${AVPLAY_ACTIVE_CLASS} body,html.${AVPLAY_ACTIVE_CLASS} #root{background:transparent!important}` +
        `#${AVPLAY_OBJECT_ID}{position:fixed;left:0;top:0;width:100%;height:100%;z-index:0}`;
      document.head.appendChild(st);
    }
    let o = document.getElementById(AVPLAY_OBJECT_ID) as HTMLObjectElement | null;
    if (!o) {
      o = document.createElement("object");
      o.id = AVPLAY_OBJECT_ID;
      o.setAttribute("type", "application/avplayer");
      const parent = this.video?.parentElement ?? document.body;
      parent.insertBefore(o, this.video ?? parent.firstChild);
    }
    this.obj = o;
    document.documentElement.classList.add(AVPLAY_ACTIVE_CLASS);
    if (this.video) this.video.style.display = "none";
  }

  private listener(): AVPlayListener {
    return {
      onbufferingstart: () => this.ev.emit("buffering", true),
      onbufferingcomplete: () => this.ev.emit("buffering", false),
      oncurrentplaytime: (ms) => {
        this.positionSec = ms / 1000;
        this.ev.emit("timeupdate", { positionSec: this.positionSec, durationSec: this.durationSec });
        this.updateExternalCue();
      },
      onstreamcompleted: () => {
        this.ev.emit("ended", undefined);
      },
      onerror: (type) => this.ev.emit("error", { message: `AVPlay error: ${type}`, fatal: true }),
      onsubtitlechange: (_dur, text) => {
        if (this.textSel?.startsWith("emb:")) this.setCue(text || null);
      },
    };
  }

  load(url: string, opts: LoadOptions = {}): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      try {
        this.closeStream();
        this.ensureObject();
        const av = this.av;
        this.url = url;
        this.positionSec = 0;
        this.audioSel = -1;
        this.textSel = null;
        this.lastCue = null;
        av.open(url);
        this.opened = true;
        av.setListener(this.listener());
        if (/\.m3u8(\?|$)/i.test(url)) {
          // Adaptive HLS: start at the highest rendition, let AVPlay adapt downwards.
          av.setStreamingProperty("ADAPTIVE_INFO", "STARTBITRATE=HIGHEST|SKIPBITRATE=LOWEST");
        }
        const cookie = opts.headers?.Cookie ?? opts.headers?.cookie;
        if (cookie) av.setStreamingProperty("COOKIE", cookie);
        try {
          av.setDisplayRect(0, 0, 1920, 1080);
          av.setDisplayMethod("PLAYER_DISPLAY_MODE_LETTER_BOX");
        } catch {
          /* display methods not critical */
        }
        this.ext = (opts.textTracks ?? []).map((t, i) => ({
          id: t.id ?? `ext:${i}`,
          label: t.label ?? (t.language ? t.language.toUpperCase() : `Subtitle ${i + 1}`),
          language: t.language,
        }));
        (opts.textTracks ?? []).forEach((t, i) => {
          fetch(t.url)
            .then((r) => r.text())
            .then((txt) => {
              this.ext[i]!.cues = parseVtt(txt);
            })
            .catch(() => {});
        });
        av.prepareAsync(
          () => {
            try {
              this.durationSec = av.getDuration() / 1000;
              this.ev.emit("ready", { durationSec: this.durationSec });
              this.ev.emit("tracks", this.getTracks());
              const finish = () => {
                if (opts.autoplay) {
                  try {
                    this.play().then(() => resolve(), reject);
                  } catch (e) {
                    reject(e);
                  }
                } else resolve();
              };
              if (opts.startPositionSec) {
                av.seekTo(opts.startPositionSec * 1000, finish, () => finish());
              } else finish();
            } catch (e) {
              reject(e);
            }
          },
          (e) => {
            const err = new Error(`AVPlay prepare failed: ${String(e)}`);
            this.ev.emit("error", { message: err.message, fatal: true });
            reject(err);
          },
        );
      } catch (e) {
        reject(e);
      }
    });
  }

  play(): Promise<void> {
    try {
      this.av.play();
      this.ev.emit("play", undefined);
      return Promise.resolve();
    } catch (e) {
      return Promise.reject(e);
    }
  }
  pause(): void {
    this.av.pause();
    this.ev.emit("pause", undefined);
  }
  seek(positionSec: number): void {
    const ms = Math.max(0, Math.round(positionSec * 1000));
    this.positionSec = positionSec;
    this.ev.emit("buffering", true);
    this.av.seekTo(
      ms,
      () => {
        this.ev.emit("buffering", false);
        this.ev.emit("timeupdate", { positionSec, durationSec: this.durationSec });
      },
      (e) => {
        this.ev.emit("buffering", false);
        this.ev.emit("error", { message: `AVPlay seek failed: ${String(e)}`, fatal: false });
      },
    );
  }
  /** Relative skip (remote ff/rw). */
  jump(deltaSec: number): void {
    this.seek(Math.min(Math.max(0, this.positionSec + deltaSec), this.durationSec || Infinity));
  }

  // ---- tracks ----
  private selectedIndex(type: string): number | undefined {
    try {
      const cur = this.av.getCurrentStreamInfo?.() ?? [];
      return cur.find((t) => t.type === type)?.index;
    } catch {
      return undefined;
    }
  }

  getTracks(): PlayerTracks {
    let info: AVPlayTrackInfo[] = [];
    try {
      info = this.av.getTotalTrackInfo() ?? [];
    } catch {
      /* not prepared */
    }
    const curAudio = this.audioSel >= 0 ? this.audioSel : this.selectedIndex("AUDIO");
    const audio: PlayerTrack[] = info
      .filter((t) => t.type === "AUDIO")
      .map((t, n) => ({
        id: `a:${t.index}`,
        ...trackLabel("Audio", n, parseExtra(t.extra_info)),
        selected: t.index === curAudio,
      }));
    if (audio.length && !audio.some((a) => a.selected)) audio[0]!.selected = true;
    const text: PlayerTrack[] = [
      ...info
        .filter((t) => t.type === "TEXT")
        .map((t, n) => ({
          id: `emb:${t.index}`,
          ...trackLabel("Subtitle", n, parseExtra(t.extra_info)),
          selected: this.textSel === `emb:${t.index}`,
        })),
      ...this.ext.map((e) => ({ id: e.id, label: e.label, language: e.language, selected: this.textSel === e.id })),
    ];
    return { audio, text };
  }

  selectAudioTrack(id: string): void {
    const idx = Number(id.replace(/^a:/, ""));
    if (Number.isNaN(idx)) return;
    this.av.setSelectTrack("AUDIO", idx);
    this.audioSel = idx;
    this.ev.emit("tracks", this.getTracks());
  }

  selectTextTrack(id: string | null): void {
    this.setCue(null);
    this.textSel = id;
    if (id === null) {
      try {
        this.av.setSilentSubtitle(true);
      } catch {
        /* ignore */
      }
    } else if (id.startsWith("emb:")) {
      const idx = Number(id.slice(4));
      this.av.setSelectTrack("TEXT", idx);
      this.av.setSilentSubtitle(false); // cues arrive through onsubtitlechange; the app renders them
    } else {
      try {
        this.av.setSilentSubtitle(true); // external: cues driven from oncurrentplaytime
      } catch {
        /* ignore */
      }
    }
    this.ev.emit("tracks", this.getTracks());
  }

  private updateExternalCue(): void {
    const sel = this.textSel;
    if (!sel || sel.startsWith("emb:")) return;
    const cues = this.ext.find((e) => e.id === sel)?.cues;
    if (!cues) return;
    const t = this.positionSec;
    this.setCue(cues.find((c) => t >= c.start && t < c.end)?.text ?? null);
  }

  private setCue(text: string | null): void {
    if (text === this.lastCue) return;
    this.lastCue = text;
    this.ev.emit("cue", { text });
  }

  // ---- lifecycle ----
  private onVisibility(): void {
    if (typeof document === "undefined") return;
    this.setSuspended(!!(document.hidden || (document as any).webkitHidden));
  }
  /** Suspend (app hidden) / restore (visible again). Also callable directly. */
  setSuspended(hidden: boolean): void {
    if (!this.opened) return;
    try {
      if (hidden && !this.suspended) {
        this.resumeAfterRestore = this.av.getState() === "PLAYING";
        this.positionSec = this.av.getCurrentTime() / 1000;
        this.av.suspend();
        this.suspended = true;
      } else if (!hidden && this.suspended) {
        this.suspended = false;
        this.av.restore(this.url, Math.round(this.positionSec * 1000));
        if (this.resumeAfterRestore) this.av.play();
      }
    } catch (e) {
      this.ev.emit("error", { message: `AVPlay suspend/restore failed: ${String(e)}`, fatal: false });
    }
  }

  private closeStream(): void {
    if (!this.opened) return;
    try {
      const av = this.av;
      if (av.getState() !== "NONE") {
        av.stop();
        av.close();
      }
    } catch {
      /* ignore */
    }
    this.opened = false;
    this.suspended = false;
  }

  destroy(): void {
    this.closeStream();
    if (typeof document !== "undefined") {
      document.removeEventListener("visibilitychange", this.onVis);
      document.removeEventListener("webkitvisibilitychange", this.onVis);
      this.obj?.remove();
      document.documentElement.classList.remove(AVPLAY_ACTIVE_CLASS);
      if (this.video) this.video.style.display = "";
    }
    this.obj = undefined;
    this.ev.clear();
  }
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void {
    return this.ev.on(type, h);
  }
}

/// <reference path="./platform/webos.d.ts" />
/// <reference path="./platform/tizen.d.ts" />
import { parseVtt, type VttCue } from "./platform/vtt";

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
  /**
   * A media download finished (hls.js fragments): network throughput from first byte to last,
   * so server-side segment generation time doesn't read as a slow link. Additive; optional.
   */
  bandwidth: { kbps: number; bytes: number; ms: number };
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

/** Standard <video> element → PlayerEventMap wiring (shared by HlsJs and WebOs adapters). */
function wireVideoEvents(v: HTMLVideoElement, ev: Emitter): void {
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
  v.addEventListener("error", () =>
    ev.emit("error", { message: v.error?.message ?? "media error", fatal: true }));
}

export class HlsJsAdapter implements PlayerAdapter {
  private hls?: import("hls.js").default;
  private ev = new Emitter();

  constructor(private video: HTMLVideoElement) {
    wireVideoEvents(video, this.ev);
  }

  async load(url: string, opts: LoadOptions = {}): Promise<void> {
    this.hls?.destroy();
    this.hls = undefined;
    const v = this.video;
    const isHls = /\.m3u8(\?|$)/.test(url);
    const nativeHls = isHls && v.canPlayType("application/vnd.apple.mpegurl") !== "";
    // hls.js is only fetched for HLS on MSE browsers; direct play stays lean.
    const Hls = isHls && !nativeHls ? (await import("hls.js")).default : undefined;
    if (Hls && Hls.isSupported()) {
      const hls = new Hls({
        startPosition: opts.startPositionSec ?? -1,
        xhrSetup: opts.headers
          ? (xhr) => Object.entries(opts.headers!).forEach(([k, val]) => xhr.setRequestHeader(k, val))
          : undefined,
      });
      this.hls = hls;
      hls.on(Hls.Events.ERROR, (_e, d) =>
        this.ev.emit("error", { message: d.details, fatal: d.fatal }));
      hls.on(Hls.Events.FRAG_LOADED, (_e, d) => {
        const st = d.frag.stats;
        // Whole download, minus a little time-to-first-byte (segment generation / RTT). Some
        // stacks deliver a slow body in one burst, so first→end alone can read as gigabits.
        const ttfb = Math.max(0, (st.loading.first || st.loading.start) - st.loading.start);
        const ms = st.loading.end - st.loading.start - Math.min(ttfb, 250);
        // Tiny or instant (cached) loads say nothing about the link.
        if (st.loaded > 128 * 1024 && ms > 20) {
          this.ev.emit("bandwidth", { kbps: (st.loaded * 8) / ms, bytes: st.loaded, ms });
        }
      });
      hls.loadSource(url);
      hls.attachMedia(v);
    } else {
      v.src = url; // direct play or native HLS (Safari, TV browsers)
      const start = opts.startPositionSec;
      if (start) {
        const seek = () => {
          v.removeEventListener("loadedmetadata", seek);
          v.currentTime = start;
        };
        v.addEventListener("loadedmetadata", seek);
      }
    }
    if (opts.autoplay) await this.play();
  }

  play(): Promise<void> {
    return this.video.play();
  }
  pause(): void {
    this.video.pause();
  }
  seek(positionSec: number): void {
    this.video.currentTime = positionSec;
  }
  destroy(): void {
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

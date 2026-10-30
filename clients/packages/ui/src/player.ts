export interface LoadOptions {
  startPositionSec?: number;
  autoplay?: boolean;
  headers?: Record<string, string>;
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
}

export type PlayerEventHandler<K extends keyof PlayerEventMap> = (data: PlayerEventMap[K]) => void;

export interface PlayerAdapter {
  load(url: string, opts?: LoadOptions): Promise<void>;
  play(): Promise<void>;
  pause(): void;
  seek(positionSec: number): void;
  destroy(): void;
  on<K extends keyof PlayerEventMap>(type: K, h: PlayerEventHandler<K>): () => void;
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

export class HlsJsAdapter implements PlayerAdapter {
  private hls?: import("hls.js").default;
  private ev = new Emitter();

  constructor(private video: HTMLVideoElement) {
    const v = video;
    v.addEventListener("timeupdate", () =>
      this.ev.emit("timeupdate", { positionSec: v.currentTime, durationSec: v.duration || 0 }));
    v.addEventListener("play", () => this.ev.emit("play", undefined));
    v.addEventListener("pause", () => this.ev.emit("pause", undefined));
    v.addEventListener("ended", () => this.ev.emit("ended", undefined));
    v.addEventListener("waiting", () => this.ev.emit("buffering", true));
    v.addEventListener("playing", () => this.ev.emit("buffering", false));
    v.addEventListener("loadedmetadata", () => this.ev.emit("ready", { durationSec: v.duration || 0 }));
    v.addEventListener("progress", () => {
      const b = v.buffered;
      let end = 0;
      for (let i = 0; i < b.length; i++) {
        if (b.start(i) <= v.currentTime + 0.5) end = Math.max(end, b.end(i));
      }
      this.ev.emit("buffered", { endSec: end });
    });
    v.addEventListener("error", () =>
      this.ev.emit("error", { message: v.error?.message ?? "media error", fatal: true }));
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

/** LG webOS: native <video> pipeline. Not implemented yet. */
export class WebOsAdapter implements PlayerAdapter {
  load(_url: string, _opts?: LoadOptions): Promise<void> { throw new Error("not implemented"); }
  play(): Promise<void> { throw new Error("not implemented"); }
  pause(): void { throw new Error("not implemented"); }
  seek(_positionSec: number): void { throw new Error("not implemented"); }
  destroy(): void { throw new Error("not implemented"); }
  on<K extends keyof PlayerEventMap>(_type: K, _h: PlayerEventHandler<K>): () => void {
    throw new Error("not implemented");
  }
}

/** Samsung Tizen AVPlay. Not implemented yet. */
export class AvPlayAdapter implements PlayerAdapter {
  load(_url: string, _opts?: LoadOptions): Promise<void> { throw new Error("not implemented"); }
  play(): Promise<void> { throw new Error("not implemented"); }
  pause(): void { throw new Error("not implemented"); }
  seek(_positionSec: number): void { throw new Error("not implemented"); }
  destroy(): void { throw new Error("not implemented"); }
  on<K extends keyof PlayerEventMap>(_type: K, _h: PlayerEventHandler<K>): () => void {
    throw new Error("not implemented");
  }
}

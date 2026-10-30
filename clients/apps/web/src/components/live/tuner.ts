/**
 * Tuning + playback for Live TV. One player adapter per page, reused across zaps; every switch
 * stops the previous session first (the server keeps that channel warm for its grace period).
 */
import { createSignal } from "solid-js";
import { AvPlayAdapter, createPlayerAdapter, mediaUrl, platform, type PlayerAdapter } from "@reel/ui";
import { ApiError } from "../../api/http";
import { iptv, type IptvChannel, type TuneBody, type TuneResponse } from "../../api/iptv";
import { clientInfo } from "../../lib/device";

export type TunePhase = "idle" | "tuning" | "playing" | "error";

export interface TuneError {
  title: string;
  body: string;
  /** Worth retrying the same channel. */
  retry: boolean;
  kind: "limit" | "offair" | "gone" | "offline" | "playback" | "other";
}

export interface ZapSample {
  channel: string;
  /** Tune request (server: reserve slot, open/attach upstream, sign URL). */
  apiMs: number;
  /** Key press / click → first painted frame. */
  ttffMs: number;
  mode: string;
  warm: boolean;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

/** Native TV players take the raw MPEG-TS; browsers get HLS (fMP4, audio fixed up when needed). */
export function tuneBody(): TuneBody {
  if (platform === "webos" || platform === "tizen") return { mode: "passthrough", preset: platform };
  return { mode: "hls", preset: clientInfo().preset };
}

export function friendlyTuneError(e: unknown): TuneError {
  const msg = e instanceof Error ? e.message : "";
  if (e instanceof ApiError) {
    if (e.status === 429) {
      const user = /user|your|per.?user|stream limit|streams? allowed/i.test(msg) && !/provider|source|connection/i.test(msg);
      return {
        kind: "limit",
        retry: true,
        title: user ? "You're at your stream limit" : "Every provider connection is in use",
        body:
          (msg && msg !== "Too many requests — try again shortly" ? msg.replace(/\.?$/, ". ") : "") +
          (user
            ? "Stop watching on another screen, then try again."
            : "Someone else is watching on another channel. Switch to a channel already playing, or try again when they stop."),
      };
    }
    if (e.status === 503)
      return { kind: "offair", retry: true, title: "This channel isn't sending any picture", body: "The provider didn't deliver a stream. It may be off air for a moment — try again, or pick another channel." };
    if (e.status === 404) return { kind: "gone", retry: false, title: "This channel is no longer available", body: "It was removed from the provider's list. Pick another channel." };
    if (e.status === 0) return { kind: "offline", retry: true, title: "Can't reach your Reel server", body: "Check your connection and try again." };
  }
  return { kind: "other", retry: true, title: "Couldn't start this channel", body: msg || "Something went wrong." };
}

export interface Tuner {
  phase: () => TunePhase;
  channelId: () => number | undefined;
  session: () => TuneResponse | undefined;
  error: () => TuneError | undefined;
  buffering: () => boolean;
  muted: () => boolean;
  unmute: () => void;
  nativePlane: () => boolean;
  samples: () => ZapSample[];
  tune: (c: IptvChannel) => Promise<void>;
  retry: () => void;
  stop: (keepalive?: boolean) => void;
  dispose: () => void;
}

export function createTuner(video: () => HTMLVideoElement | undefined): Tuner {
  const [phase, setPhase] = createSignal<TunePhase>("idle");
  const [channelId, setChannelId] = createSignal<number>();
  const [session, setSession] = createSignal<TuneResponse>();
  const [error, setError] = createSignal<TuneError>();
  const [buffering, setBuffering] = createSignal(false);
  const [muted, setMuted] = createSignal(false);
  const [nativePlane, setNativePlane] = createSignal(false);
  const [samples, setSamples] = createSignal<ZapSample[]>([]);
  let adapter: PlayerAdapter | undefined;
  let gen = 0;
  let last: IptvChannel | undefined;
  const seen = new Set<number>();

  const ensureAdapter = (): PlayerAdapter | undefined => {
    if (adapter) return adapter;
    const v = video();
    if (!v) return undefined;
    const a = createPlayerAdapter(v);
    adapter = a;
    setNativePlane(a instanceof AvPlayAdapter);
    a.on("buffering", (b) => setBuffering(b));
    a.on("play", () => setBuffering(false));
    a.on("error", (e) => {
      if (!e.fatal || phase() === "idle") return;
      setPhase("error");
      setBuffering(false);
      setError({ kind: "playback", retry: true, title: "The picture stopped", body: "The stream broke off. Try again, or pick another channel." });
    });
    return a;
  };

  const endSession = (keepalive = false) => {
    const s = session();
    setSession(undefined);
    if (s) return iptv.stop(s.session_id, keepalive).catch(() => undefined);
    return Promise.resolve();
  };

  /** Resolves on the first painted frame (requestVideoFrameCallback where available). */
  const firstFrame = (): Promise<void> => {
    const v = video() as (HTMLVideoElement & { requestVideoFrameCallback?: (cb: () => void) => number }) | undefined;
    if (!v || nativePlane() || typeof v.requestVideoFrameCallback !== "function") return Promise.resolve();
    return Promise.race([new Promise<void>((r) => v.requestVideoFrameCallback!(() => r())), sleep(4000)]);
  };

  const tune = async (c: IptvChannel): Promise<void> => {
    const my = ++gen;
    const t0 = now();
    last = c;
    setChannelId(c.id);
    setError(undefined);
    setPhase("tuning");
    setBuffering(true);
    // Free our slot before asking for a new one (user stream limits); don't wait long.
    await Promise.race([endSession(), sleep(700)]);
    if (my !== gen) return;
    let r: TuneResponse;
    try {
      r = await iptv.tune(c.id, tuneBody());
    } catch (e) {
      if (my !== gen) return;
      // The previous session is already stopped: don't leave its last frames running.
      try {
        video()?.pause();
      } catch {
        /* ignore */
      }
      setBuffering(false);
      setPhase("error");
      setError(friendlyTuneError(e));
      return;
    }
    if (my !== gen) {
      iptv.stop(r.session_id).catch(() => undefined);
      return;
    }
    const apiMs = now() - t0;
    setSession(r);
    const a = ensureAdapter();
    if (!a) return;
    const v = video();
    try {
      await a.load(mediaUrl(r.url)!, { autoplay: true });
    } catch (e) {
      if (my !== gen) return;
      // Browsers block autoplay with sound until the first interaction: carry on muted.
      if (v && e instanceof Error && e.name === "NotAllowedError") {
        v.muted = true;
        setMuted(true);
        try {
          await a.play();
        } catch {
          /* still blocked — the overlay offers a play button */
        }
      } else if (!(e instanceof Error && e.name === "AbortError")) {
        setPhase("error");
        setBuffering(false);
        setError({ kind: "playback", retry: true, title: "Couldn't play this channel", body: e instanceof Error ? e.message : "The player refused the stream." });
        return;
      }
    }
    if (my !== gen) return;
    await firstFrame();
    if (my !== gen) return;
    setPhase("playing");
    setBuffering(false);
    const warm = seen.has(c.id);
    seen.add(c.id);
    setSamples((l) => ([{ channel: c.name, apiMs: Math.round(apiMs), ttffMs: Math.round(now() - t0), mode: r.mode, warm }] as ZapSample[]).concat(l).slice(0, 12));
  };

  return {
    phase, channelId, session, error, buffering, muted, nativePlane, samples, tune,
    unmute: () => {
      const v = video();
      if (v) {
        v.muted = false;
        v.play().catch(() => undefined);
      }
      setMuted(false);
    },
    retry: () => {
      if (last) void tune(last);
    },
    stop: (keepalive) => {
      gen++;
      void endSession(keepalive);
      setPhase("idle");
      setBuffering(false);
    },
    dispose: () => {
      gen++;
      void endSession(true);
      adapter?.destroy();
      adapter = undefined;
    },
  };
}

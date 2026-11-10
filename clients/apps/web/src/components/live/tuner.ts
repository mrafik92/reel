/**
 * Tuning + playback for Live TV. One player adapter per page, reused across zaps; every switch
 * stops the previous session first (the server keeps that channel warm for its grace period).
 */
import { createSignal } from "solid-js";
import { AvPlayAdapter, createPlayerAdapter, detectDeviceProfile, mediaUrl, platform, type DeviceProfile, type PlayerAdapter } from "@reel/ui";
import { ApiError } from "../../api/http";
import { iptv, type IptvChannel, type TuneBody, type TuneResponse } from "../../api/iptv";
import { clientInfo } from "../../lib/device";
import { wantsDirect } from "../../lib/liveDelivery";
import { toast } from "../../lib/toast";

export type TunePhase = "idle" | "tuning" | "playing" | "error";

export interface TuneError {
  title: string;
  body: string;
  /** Worth retrying the same channel. */
  retry: boolean;
  kind: "limit" | "offair" | "gone" | "offline" | "playback" | "other";
  /** 429 from the provider budget: channels currently holding the connections. */
  watching?: string[];
  /** 429 from the provider budget (vs. the user's own stream limit). */
  provider?: boolean;
  /** Admins (409): the other users' sessions holding every connection — can be taken over. */
  holders?: Holder[];
}

export interface Holder {
  session_id: string;
  user_id: number;
  username?: string | null;
  channel_id?: number;
  channel_name: string;
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

/** A direct session needs a heartbeat: the server never sees its video. It drops it after ~30 s of silence. */
const HEARTBEAT_MS = 15_000;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

type WireProfile = NonNullable<TuneBody["profile"]>;

/** Browser probe → the server's DeviceProfile, so HLS can copy every codec this browser decodes (HEVC included). */
function fromWebProbe(p: DeviceProfile): WireProfile {
  const depth = p.bitDepth ?? {};
  const video = p.video.map((codec) => ({
    codec,
    max_bit_depth: depth[codec] ?? (codec === "h264" ? 8 : 10),
    hdr: [] as never[],
  }));
  const audio: { codec: string; max_channels: number }[] = p.audio.map((codec) => ({ codec, max_channels: codec === "aac" || codec === "opus" ? 8 : 6 }));
  if (!audio.some((a) => a.codec === "mp3")) audio.push({ codec: "mp3", max_channels: 2 });
  return {
    name: `${clientInfo().preset}-live`,
    player: typeof MediaSource !== "undefined" ? "mse" : "native",
    containers: ["mp4"],
    video,
    audio,
    subtitles: { text: ["vtt"], image: [] },
    max_width: 3840,
    max_height: 2160,
  } as unknown as WireProfile;
}

/**
 * Native TV players take the raw MPEG-TS (passthrough); browsers get HLS. Either way the real device
 * profile goes along so the server only transcodes what this device can't decode.
 */
export function tuneBody(o: { viaServer?: boolean } = {}): TuneBody {
  // This device plays straight from the provider (Settings → Playback); the server only holds the slot.
  if (!o.viaServer && wantsDirect()) return { mode: "direct" };
  let raw: ReturnType<typeof detectDeviceProfile> | undefined;
  try {
    raw = detectDeviceProfile();
  } catch {
    raw = undefined;
  }
  if (platform === "webos" || platform === "tizen") {
    if (raw && "profile" in raw) return { mode: "passthrough", profile: raw.profile as unknown as WireProfile };
    return { mode: "passthrough", preset: raw && "preset" in raw ? raw.preset : platform };
  }
  const body: TuneBody = { mode: "hls", preset: clientInfo().preset };
  if (raw && "video" in raw) body.profile = fromWebProbe(raw);
  return body;
}

const plan = (max?: number) => (max ? `Your TV plan allows ${max} stream${max === 1 ? "" : "s"}` : "Your TV plan's streams are all in use");

export function friendlyTuneError(e: unknown): TuneError {
  const msg = e instanceof Error ? e.message : "";
  if (e instanceof ApiError) {
    const body = (e as ApiError & { body?: Record<string, unknown> }).body;
    const maxOf = (m: string) => Number((/All (\d+) connection/i.exec(m) ?? /\((\d+) concurrent\)/i.exec(m) ?? [])[1]) || undefined;
    // Admins: every connection is held by other users → they can take it over.
    if (e.status === 409 && body && Array.isArray(body.holders)) {
      const holders = (body.holders as Holder[]).filter((h) => h && h.session_id);
      const max = maxOf(msg) ?? (holders.length || undefined);
      // "sam is watching Arena One and Cine Classics" — one clause per person.
      const byUser = new Map<string, string[]>();
      holders.forEach((h) => {
        const k = h.username ?? "Someone";
        byUser.set(k, (byUser.get(k) ?? []).concat(h.channel_name));
      });
      const and = (l: string[]) => (l.length > 1 ? `${l.slice(0, -1).join(", ")} and ${l[l.length - 1]}` : l[0] ?? "");
      const who = Array.from(byUser, ([u, chans]) => `${u} is watching ${and(chans)}`).join("; ");
      return {
        kind: "limit",
        provider: true,
        retry: true,
        holders,
        watching: holders.map((h) => h.channel_name),
        title: who ? `${plan(max)} — ${who}` : plan(max),
        body: "Take over to stop their stream and watch this channel here, or wait until they're done.",
      };
    }
    if (e.status === 429) {
      const user = /reached your limit|concurrent stream\(s\)|stream limit/i.test(msg) && !/provider|source|connection/i.test(msg);
      if (user)
        return {
          kind: "limit",
          retry: true,
          title: "You're at your stream limit",
          body: "Your account can only play so many streams at once. Stop watching on another screen, then try again.",
        };
      // Other viewers hold the provider's connections (non-admins get no names). Older servers
      // said "connection limit of source 1 reached (1 concurrent); in use by: MBC Masr FHD".
      const by = /in use by:\s*(.+)$/i.exec(msg);
      const watching = by ? by[1]!.split(/,\s*/).map((x) => x.trim()).filter(Boolean) : undefined;
      const max = maxOf(msg);
      return {
        kind: "limit",
        provider: true,
        retry: true,
        watching,
        title: watching?.length ? `${plan(max)} — someone is watching ${watching.join(", ")}` : plan(max),
        body: "Someone else is watching another channel right now. Try again when they're done, or ask an admin to free the connection.",
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
  tune: (c: IptvChannel, o?: { takeover?: boolean; viaServer?: boolean }) => Promise<void>;
  /** `takeover`: admins stop the other users' streams holding the provider connections. */
  retry: (o?: { takeover?: boolean }) => void;
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
  let beat: ReturnType<typeof setInterval> | undefined;
  const stopBeat = () => {
    if (beat) clearInterval(beat);
    beat = undefined;
  };
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
      if (session()?.mode === "direct" && fallBack()) return;
      setPhase("error");
      setBuffering(false);
      setError({ kind: "playback", retry: true, title: "The picture stopped", body: "The stream broke off. Try again, or pick another channel." });
    });
    return a;
  };

  /** Direct playback failed on this device (CORS, format, provider refused): use the server instead, once. */
  const fallBack = (): boolean => {
    if (!last) return false;
    toast("Couldn't play directly from the provider on this device — using the server instead.", "info", 6000);
    void tune(last, { viaServer: true });
    return true;
  };

  const endSession = (keepalive = false) => {
    stopBeat();
    const s = session();
    setSession(undefined);
    if (s) forgetSession(s.session_id);
    if (s) return iptv.stop(s.session_id, keepalive).catch(() => undefined);
    return Promise.resolve();
  };

  /** Resolves on the first painted frame (requestVideoFrameCallback where available). */
  const firstFrame = (): Promise<void> => {
    const v = video() as (HTMLVideoElement & { requestVideoFrameCallback?: (cb: () => void) => number }) | undefined;
    if (!v || nativePlane() || typeof v.requestVideoFrameCallback !== "function") return Promise.resolve();
    return Promise.race([new Promise<void>((r) => v.requestVideoFrameCallback!(() => r())), sleep(4000)]);
  };

  const tune = async (c: IptvChannel, o: { takeover?: boolean; viaServer?: boolean } = {}): Promise<void> => {
    const my = ++gen;
    const t0 = now();
    last = c;
    setChannelId(c.id);
    setError(undefined);
    setPhase("tuning");
    setBuffering(true);
    // Release our slot before asking for a new one; wait at most 500 ms.
    await Promise.race([endSession(), sleep(500)]);
    if (my !== gen) return;
    let r: TuneResponse;
    try {
      const body = tuneBody({ viaServer: o.viaServer });
      r = await iptv.tune(c.id, o.takeover ? ({ ...body, takeover: true } as TuneBody) : body);
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
    rememberSession(r);
    if (r.mode === "direct") {
      const sid = r.session_id;
      beat = setInterval(() => {
        iptv.keepalive(sid).catch((e) => {
          // 404: the server ended it (taken over, or we were silent too long). Anything else: try again next beat.
          if (!(e instanceof ApiError) || e.status !== 404 || session()?.session_id !== sid) return;
          stopBeat();
          setSession(undefined);
          forgetSession(sid);
          try {
            video()?.pause();
          } catch {
            /* ignore */
          }
          setPhase("error");
          setBuffering(false);
          setError({ kind: "gone", retry: true, title: "Your stream was stopped", body: "Another viewer took over the provider connection, or this device lost touch with Reel. Try again." });
        });
      }, HEARTBEAT_MS);
    }
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
        if (r.mode === "direct" && fallBack()) return;
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
    retry: (o) => {
      if (last) void tune(last, o);
    },
    stop: (keepalive) => {
      gen++;
      void endSession(keepalive);
      try {
        video()?.pause();
      } catch {
        /* ignore */
      }
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

// ---- this browser's last live session (to offer "stop the other stream" after a reload) ----
const SESSION_KEY = "reel.live.session";
export interface OwnSession {
  id: string;
  channel: string;
}
function rememberSession(r: TuneResponse): void {
  try {
    localStorage.setItem(SESSION_KEY, JSON.stringify({ id: r.session_id, channel: r.channel_name }));
  } catch {
    /* ignore */
  }
}
export function forgetSession(id?: string): void {
  try {
    const cur = ownSession();
    if (!id || (cur && cur.id === id)) localStorage.removeItem(SESSION_KEY);
  } catch {
    /* ignore */
  }
}
export function ownSession(): OwnSession | undefined {
  try {
    const v = JSON.parse(localStorage.getItem(SESSION_KEY) || "null") as OwnSession | null;
    return v && v.id ? v : undefined;
  } catch {
    return undefined;
  }
}

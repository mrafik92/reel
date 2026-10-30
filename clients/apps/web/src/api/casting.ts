/**
 * Casting endpoints.
 *
 *  - DLNA "Play on TV" is server-driven: the server discovers renderers (SSDP) and pushes
 *    the stream to them; the browser only sends commands and polls the state.
 *  - Chromecast / AirPlay receivers fetch media themselves: decide / vod play / tune with
 *    `for_cast: true` return absolute, long-lived URLs on a LAN-reachable base, and
 *    `POST /playback/{id}/cast-url` converts an already running session.
 *
 * Hand-written until the server's OpenAPI includes these. `?mock=1` → in-memory fakes.
 */
import { MOCK } from "../lib/mode";
import { ApiError, http } from "./http";

/** `CastDeviceDto` — a DLNA TV the server found (SSDP) or an admin added by IP. */
export interface CastDevice {
  id: string;
  name: string;
  ip: string;
  /** Always "dlna" for now. */
  protocol: string;
  manufacturer?: string | null;
  model?: string | null;
  /** Absolute icon URL on the device. */
  icon?: string | null;
  supports_volume: boolean;
  manual: boolean;
}

export interface CastPlayBody {
  kind: "item" | "vod" | "channel";
  /** Library: file id (or use `file_id`). VOD: movie/episode id. Channel: channel id. */
  id?: number | string | null;
  /** Library: the version to play. */
  file_id?: number | null;
  /** VOD: movie or episode. */
  vod_kind?: "movie" | "episode";
  start_s?: number | null;
  audio_index?: number | null;
  /** null = automatic, -1 = off */
  subtitle_index?: number | null;
}

export type CastState = "playing" | "paused" | "stopped" | "buffering";

export interface CastSessionState {
  state: CastState;
  position_s: number;
  duration_s?: number | null;
  /** 0..100 when the renderer reports it (not in the current API). */
  volume?: number | null;
}

export interface CastUrlResponse {
  session_id: string;
  url: string;
  content_type: string;
  base_url: string;
  expires_in_s: number;
  subtitles: { index: number; url?: string | null; language?: string | null; forced: boolean }[];
}

export interface CastingApi {
  devices(): Promise<CastDevice[]>;
  play(deviceId: string, body: CastPlayBody): Promise<{ cast_session_id: string }>;
  session(id: string): Promise<CastSessionState>;
  control(id: string, action: "play" | "pause" | "stop" | "seek", positionS?: number): Promise<void>;
  /** `level` 0..100 */
  volume(id: string, level: number): Promise<void>;
  /** Absolute receiver URLs for a running playback session. */
  castUrl(sessionId: string): Promise<CastUrlResponse>;
}

const enc = encodeURIComponent;

const httpCasting: CastingApi = {
  devices: async () => {
    let v: unknown;
    try {
      v = await http.get<unknown>("/cast/devices");
    } catch (e) {
      // A non-JSON answer (the web app's HTML) means the route doesn't exist on this server.
      if (e instanceof ApiError) throw e;
      throw new ApiError(404, "Not supported by this server");
    }
    // Older servers answer unknown routes with the web app's HTML.
    if (!Array.isArray(v)) throw new ApiError(404, "Not supported by this server");
    return v as CastDevice[];
  },
  play: (id, body) => http.post(`/cast/devices/${enc(id)}/play`, body),
  session: (id) => http.get(`/cast/sessions/${enc(id)}`),
  control: (id, action, positionS) =>
    http.post(`/cast/sessions/${enc(id)}/${action}`, action === "seek" ? { position_s: positionS ?? 0 } : undefined),
  volume: (id, level) => http.post(`/cast/sessions/${enc(id)}/volume`, { level: Math.round(level) }),
  castUrl: (sid) => http.post(`/playback/${enc(sid)}/cast-url`),
};

// ---- mock: two renderers and a simulated playhead ---------------------------
interface MockSession { state: CastState; pos: number; dur: number; at: number; vol: number }
const sessions = new Map<string, MockSession>();
const tick = (s: MockSession) => {
  const now = Date.now();
  if (s.state === "playing") s.pos = Math.min(s.dur, s.pos + (now - s.at) / 1000);
  if (s.pos >= s.dur && s.dur > 0) s.state = "stopped";
  s.at = now;
};
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const mockCasting: CastingApi = {
  async devices() {
    await wait(450);
    return [
      { id: "uuid:lg-oled", name: "Living Room TV", ip: "192.168.1.40", protocol: "dlna", manufacturer: "LG", model: "OLED65C1", supports_volume: true, manual: false },
      { id: "uuid:samsung-q", name: "Bedroom", ip: "192.168.1.41", protocol: "dlna", manufacturer: "Samsung", model: "QE55Q80", supports_volume: false, manual: false },
    ];
  },
  async play(_id, body) {
    await wait(700);
    const id = `cast-${Math.random().toString(36).slice(2, 9)}`;
    const dur = body.kind === "channel" ? 0 : 6200;
    sessions.set(id, { state: "buffering", pos: body.start_s ?? 0, dur, at: Date.now(), vol: 40 });
    setTimeout(() => {
      const s = sessions.get(id);
      if (s && s.state === "buffering") {
        s.state = "playing";
        s.at = Date.now();
      }
    }, 1500);
    return { cast_session_id: id };
  },
  async session(id) {
    await wait(60);
    const s = sessions.get(id);
    if (!s) return { state: "stopped", position_s: 0, duration_s: 0 };
    tick(s);
    return { state: s.state, position_s: s.pos, duration_s: s.dur, volume: s.vol };
  },
  async control(id, action, p) {
    await wait(120);
    const s = sessions.get(id);
    if (!s) return;
    tick(s);
    if (action === "play") s.state = "playing";
    else if (action === "pause") s.state = "paused";
    else if (action === "stop") s.state = "stopped";
    else if (action === "seek") s.pos = Math.max(0, p ?? 0);
  },
  async volume(id, level) {
    await wait(80);
    const s = sessions.get(id);
    if (s) s.vol = level;
  },
  async castUrl(sid) {
    await wait(150);
    return {
      session_id: sid,
      url: `http://192.168.1.10:8420/api/v1/stream/${sid}/master.m3u8?sig=mock`,
      content_type: "application/vnd.apple.mpegurl",
      base_url: "http://192.168.1.10:8420",
      expires_in_s: 8 * 3600,
      subtitles: [],
    };
  },
};

export const casting: CastingApi = MOCK ? mockCasting : httpCasting;

import { detectDeviceProfile, type Id } from "@reel/ui";
import { api, type DecideRequest, type DecideResponse, type DeviceProfileWire, type Quality } from "../api";
import { clientInfo, deviceId } from "./device";

/**
 * Device profile for `POST /playback/{file_id}/decide`.
 *
 * `detectDeviceProfile()` (platform lane) returns `{preset}` / `{profile}` on TVs and
 * the probed browser capabilities on the web; the latter is converted to the
 * server's DeviceProfile shape here.
 */
export function decideProfile(): Pick<DecideRequest, "preset" | "profile"> {
  let raw: ReturnType<typeof detectDeviceProfile> | undefined;
  try {
    raw = detectDeviceProfile();
  } catch {
    raw = undefined;
  }
  if (!raw) return { preset: clientInfo().preset };
  // TVs: `{preset}` or a refined `{profile}` (ServerDeviceProfile) — pass straight through.
  if ("preset" in raw) return { preset: raw.preset };
  if ("profile" in raw) return { profile: raw.profile as unknown as DeviceProfileWire };
  // Web: probed browser capabilities → server DeviceProfile, plus the preset as a fallback.
  return { preset: clientInfo().preset, profile: fromBrowserProbe(raw as unknown as Record<string, unknown>) };
}

function fromBrowserProbe(r: Record<string, unknown>): DeviceProfileWire {
  const video = (r.video as string[]).map((codec) => ({
    codec,
    max_bit_depth: codec === "h264" ? 8 : 10,
    hdr: [] as ("none" | "hdr10" | "hlg" | "dv")[],
  }));
  const audio = (r.audio as string[]).map((codec) => ({
    codec,
    // browsers downmix multichannel AAC/Opus themselves
    max_channels: codec === "aac" || codec === "opus" ? 8 : 6,
  }));
  // Browsers decode MP3/FLAC everywhere even though the probe doesn't list them.
  if (!audio.some((a) => a.codec === "mp3")) audio.push({ codec: "mp3", max_channels: 2 });
  if (!audio.some((a) => a.codec === "flac")) audio.push({ codec: "flac", max_channels: 6 });
  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
  const h = typeof screen !== "undefined" ? Math.round(screen.height * dpr) : 1080;
  const maxH = Math.min(Number(r.maxHeight) || 2160, h >= 2000 ? 2160 : h >= 1400 ? 1440 : 1080);
  return {
    name: `${clientInfo().client}-browser`,
    player: typeof MediaSource !== "undefined" ? "mse" : "native",
    containers: (r.containers as string[]) ?? ["mp4"],
    video,
    audio,
    subtitles: { text: ["vtt"], image: [] },
    max_width: Math.round((maxH * 16) / 9),
    max_height: maxH,
  };
}

/**
 * `itemId`: the episode being watched — required for multi-episode files so progress lands on it.
 * Tracks: `null` / omitted = let the server choose from the user's preferences (`selected` in the
 * response), `subtitle: -1` = explicitly off. `forCast`: the stream goes to a Chromecast / AirPlay
 * receiver (absolute, long-lived URLs; receiver profile instead of this browser's). `quality`: the
 * device default or the player's pick (#87); omitted = the server's Auto.
 */
export async function decide(
  fileId: Id,
  o: { itemId?: Id | null; audio?: number | null; subtitle?: number | null; start?: number; forCast?: "chromecast" | "airplay"; quality?: Quality },
): Promise<DecideResponse> {
  const body: DecideRequest = {
    ...(o.forCast ? { preset: o.forCast } : decideProfile()),
    item_id: o.itemId ?? null,
    device_id: deviceId(),
    audio_index: o.audio ?? null,
    subtitle_index: o.subtitle ?? null,
    start_s: o.start && o.start > 0 ? o.start : null,
  };
  if (o.forCast) body.for_cast = true;
  if (o.quality) body.quality = o.quality;
  return api.decide(fileId, body);
}

export const MODE_LABEL: Record<string, string> = {
  direct_play: "Direct play",
  remux: "Direct stream",
  audio_transcode: "Audio transcode",
  transcode: "Transcode",
};

const PREF_KEY = "reel.tracks";
export interface TrackPrefs {
  audio?: number;
  /** -1 = off */
  sub?: number;
}
/** Per-item audio/subtitle choice, remembered on this device. */
export function getTrackPrefs(itemId: Id): TrackPrefs {
  try {
    const all = JSON.parse(localStorage.getItem(PREF_KEY) || "{}") as Record<string, TrackPrefs>;
    return all[String(itemId)] ?? {};
  } catch {
    return {};
  }
}
export function setTrackPrefs(itemId: Id, p: TrackPrefs): void {
  try {
    const all = JSON.parse(localStorage.getItem(PREF_KEY) || "{}") as Record<string, TrackPrefs>;
    all[String(itemId)] = { ...all[String(itemId)], ...p };
    localStorage.setItem(PREF_KEY, JSON.stringify(all));
  } catch {
    /* ignore */
  }
}

/** The `item_id` to send to decide: the episode, when this file holds several. */
export function creditItem(file: { item_ids?: Id[] | null }, itemId: Id): Id | null {
  const ids = file.item_ids;
  return ids && ids.length > 1 && ids.indexOf(itemId) >= 0 ? itemId : null;
}

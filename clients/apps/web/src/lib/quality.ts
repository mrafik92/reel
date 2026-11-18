import { createSignal } from "solid-js";
import type { DecideResponse, Quality, QualityOption } from "../api";
import { currentUser } from "./auth";

/**
 * Playback quality (#87): the per-device default (Settings → Playback → Quality) and the helpers the
 * player uses to explain what the server did with it. The server resolves the choice
 * (`server/crates/playback/src/quality.rs`): Auto = original when it fits the bandwidth cap, else the
 * best ladder rung; Original = never re-encode the picture, unless the device can't play it or the
 * user's bitrate limit is lower; a rung = cap at that size (never upscales).
 *
 * Per device, like the Live TV delivery choice: the right answer depends on the connection.
 * Multi-variant sessions (#111, hls.js players) list every variant whatever the choice; the player
 * applies it locally (`lib/abr.ts`): Auto = ABR, Original = pin, a rung = ABR capped at that size.
 */

export const QUALITIES: Quality[] = ["auto", "original", "1080p", "720p", "480p"];

export const QUALITY_NAME: Record<Quality, string> = {
  auto: "Auto",
  original: "Original",
  "1080p": "1080p",
  "720p": "720p",
  "480p": "480p",
};

const KEY = "reel.playback.quality";

function read(): Quality {
  try {
    const v = localStorage.getItem(KEY) as Quality | null;
    return v && QUALITIES.indexOf(v) >= 0 ? v : "auto";
  } catch {
    return "auto";
  }
}

const [deviceQuality, setDeviceQuality] = createSignal<Quality>(read());

/** The default quality on this device (what every play asks for unless the player overrides it). */
export { deviceQuality };

export function chooseDeviceQuality(q: Quality): void {
  setDeviceQuality(q);
  try {
    if (q === "auto") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, q);
  } catch {
    /* storage disabled: the choice lasts until reload */
  }
}

/** The signed-in user's bitrate limit (kbit/s), or null when there is none. */
export const userLimitKbps = (): number | null => currentUser()?.limits?.max_bitrate_kbps ?? null;

export function fmtMbps(kbps: number | null | undefined): string {
  if (!kbps || !isFinite(kbps)) return "—";
  return kbps >= 10_000 ? `${Math.round(kbps / 1000)} Mbps` : `${(kbps / 1000).toFixed(1)} Mbps`;
}

/** The session's quality options in ladder order, or [] for servers that don't send them. */
export function qualityOptions(d: DecideResponse | undefined): QualityOption[] {
  return d?.quality_options ?? [];
}

/** The picture is re-encoded (a full transcode), as opposed to direct play / remux / audio-only. */
export function videoTranscoded(d: DecideResponse | undefined): boolean {
  return d?.decision.video.type === "encode";
}

/** The `adaptive quality …` reason the server adds when it picked a ladder rung. */
const adaptiveReason = (d: DecideResponse) => d.decision.reasons.find((r) => r.indexOf("adaptive quality") === 0);

/** "source needs ~25.0 Mbps but the cap is 6.0 Mbps (user limit)" → its parts. */
function capReason(d: DecideResponse): { needs: string; cap: string; by: "user limit" | "client bandwidth" } | undefined {
  const m = /source needs ~([\d.]+ Mbps) but the cap is ([\d.]+ Mbps) \((user limit|client bandwidth)\)/.exec(adaptiveReason(d) ?? "");
  return m ? { needs: m[1]!, cap: m[2]!, by: m[3] as "user limit" | "client bandwidth" } : undefined;
}

/** Why the server had to re-encode the picture, in plain words (for the "Original" prompt). */
export function whyTranscode(d: DecideResponse): string {
  const cap = capReason(d);
  if (cap?.by === "user limit") return `The original needs about ${cap.needs}, but your account is limited to ${cap.cap}.`;
  if (d.decision.subtitle.type === "burn_in") return "The subtitles you picked are pictures, so they have to be drawn into the video.";
  const v = d.decision.video;
  const r = d.decision.reasons;
  if (v.type === "encode" && v.tonemap) return "This is an HDR video and this screen can't show HDR, so it has to be converted to SDR.";
  if (r.some((x) => /^resolution .* exceeds client max/.test(x))) return "The video is bigger than this device can show.";
  if (r.some((x) => /^video codec|profile '|level .* exceeds|bit depth/.test(x))) return "This device can't decode the original video format.";
  return "This device can't play the original video as it is.";
}

/** Multi-variant session (#111): hls.js switches between the listed variants. */
const multiVariant = (d: DecideResponse | undefined) => !!d?.start_variant && (d.variants?.length ?? 0) > 1;

/**
 * One line for stream info: what happened and why (direct play / remux / transcode). `asked` is the
 * quality this play requested. Multi-variant sessions describe their best version, then the switching.
 */
export function explainStream(d: DecideResponse, asked: Quality): string {
  const base = explainTop(d, asked);
  if (!multiVariant(d)) return base;
  const n = d.variants!.length;
  return `${base} The player switches between ${n} versions as the connection changes.`;
}

function explainTop(d: DecideResponse, asked: Quality): string {
  switch (d.mode) {
    case "direct_play":
      return "Playing the original file as it is.";
    case "remux":
      return "Original picture and sound, repackaged for this player. Nothing is re-encoded.";
    case "audio_transcode":
      return "Original picture. Only the sound is converted, because this device can't play the original audio.";
  }
  const q = d.quality && d.quality !== "original" ? d.quality : undefined;
  const to = q ? `Converted to ${q}` : "Converted";
  const cap = capReason(d);
  if (cap?.by === "user limit") return `${to}: the original needs about ${cap.needs}, but your account is limited to ${cap.cap}.`;
  if (cap) return `${to}: the original needs about ${cap.needs}, more than this device asked for (${cap.cap}).`;
  if (q && asked === q) return `${to} because you chose ${q}.`;
  return `${to}. ${whyTranscode(d)}`;
}

/**
 * Detail line for one quality option in the player's picker. `playing`: what plays now (the
 * variant hls.js picked, for a multi-variant session).
 */
export function optionDetail(o: QualityOption | undefined, q: Quality, d: DecideResponse | undefined, playing?: Quality): string {
  const limit = userLimitKbps();
  if (q === "auto" && multiVariant(d)) {
    const v = playing ? d!.variants!.find((x) => x.id === playing) : undefined;
    const now = v ? ` · now ${v.label}, ${fmtMbps(v.average_kbps)}` : "";
    return `Switches between ${d!.variants!.length} versions as your connection changes${now}`;
  }
  if (q === "auto") {
    const now = d?.quality ? ` · now ${QUALITY_NAME[d.quality]}${d.bitrate_kbps ? `, ${fmtMbps(d.bitrate_kbps)}` : ""}` : "";
    return `Original when it fits${limit ? ` your ${fmtMbps(limit)} limit` : ""}, else the best size that does${now}`;
  }
  if (!o) return "Not offered for this title";
  const rate = fmtMbps(o.bitrate_kbps);
  if (!o.available) {
    if (limit && o.bitrate_kbps > limit) return `${rate} · limited to ${fmtMbps(limit)} by your account`;
    return q === "original" ? "Not available" : "No smaller than the original";
  }
  if (q === "original") {
    // The current stream re-encodes the picture without a quality reason: so would Original.
    const forced = d && videoTranscoded(d) && !adaptiveReason(d);
    return forced ? `${rate} · this device can't play it without converting` : `${rate} · no re-encoding`;
  }
  return rate;
}

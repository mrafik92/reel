/// <reference path="./platform/tizen.d.ts" />
import { AvPlayAdapter, HlsJsAdapter, WebOsAdapter, type PlayerAdapter } from "./player";
import { detectPlatform, platform } from "./platform/detect";

export type VideoCodec = "h264" | "hevc" | "av1" | "vp9";
export type AudioCodec = "aac" | "ac3" | "eac3" | "opus";

export interface DeviceProfile {
  platform: "web" | "webos" | "tizen";
  video: VideoCodec[];
  audio: AudioCodec[];
  containers: string[];
  maxHeight?: number;
}

const VIDEO_MIME: Record<VideoCodec, string[]> = {
  h264: ['video/mp4; codecs="avc1.640028"'],
  hevc: ['video/mp4; codecs="hvc1.1.6.L120.B0"', 'video/mp4; codecs="hev1.1.6.L120.B0"'],
  av1: ['video/mp4; codecs="av01.0.08M.08"'],
  vp9: ['video/webm; codecs="vp9"', 'video/mp4; codecs="vp09.00.40.08"'],
};
const AUDIO_MIME: Record<AudioCodec, string[]> = {
  aac: ['audio/mp4; codecs="mp4a.40.2"'],
  ac3: ['audio/mp4; codecs="ac-3"'],
  eac3: ['audio/mp4; codecs="ec-3"'],
  opus: ['audio/webm; codecs="opus"', 'audio/mp4; codecs="opus"'],
};

function supports(mimes: string[]): boolean {
  const ms = typeof MediaSource !== "undefined" ? MediaSource : undefined;
  let el: HTMLVideoElement | undefined;
  return mimes.some((m) => {
    if (ms && typeof ms.isTypeSupported === "function" && ms.isTypeSupported(m)) return true;
    if (!el && typeof document !== "undefined") el = document.createElement("video");
    return !!el && el.canPlayType(m) !== "";
  });
}

export function detectWebProfile(platform: DeviceProfile["platform"] = "web"): DeviceProfile {
  const video = (Object.keys(VIDEO_MIME) as VideoCodec[]).filter((k) => supports(VIDEO_MIME[k]));
  const audio = (Object.keys(AUDIO_MIME) as AudioCodec[]).filter((k) => supports(AUDIO_MIME[k]));
  const containers = ["mp4"];
  if (supports(['video/webm; codecs="vp9"', 'video/webm; codecs="vp8"'])) containers.push("webm");
  if (supports(["video/x-matroska"])) containers.push("mkv");
  return { platform, video, audio, containers };
}

// ---------------------------------------------------------------------------------------------
// TV device profile detection + player factory
// ---------------------------------------------------------------------------------------------

/** Body fragment for POST /api/v1/playback/{file_id}/decide. */
export type DeviceProfileRequest = { preset: string } | { profile: ServerDeviceProfile };

/** Mirror of the server's `DeviceProfile` (crates/playback/src/profile.rs). */
export interface ServerDeviceProfile {
  name: string;
  player: "mse" | "native";
  containers: string[];
  video: { codec: string; max_profile?: string | null; max_level?: number | null; max_bit_depth: number; hdr: string[] }[];
  audio: { codec: string; max_channels: number }[];
  subtitles: { text: string[]; image: string[] };
  max_width: number;
  max_height: number;
  max_bitrate_kbps?: number | null;
}

export interface TvDeviceInfo {
  platform: "webos" | "tizen";
  model?: string;
  firmware?: string;
  /** Panel is UHD (3840x2160) capable. Undefined = unknown. */
  uhd?: boolean;
  uhd8k?: boolean;
  /** Panel can display HDR10. Undefined = unknown. */
  hdr?: boolean;
  dolbyVision?: boolean;
}

/** Synchronous capability probe via vendor APIs (all guarded). */
export function detectTvInfo(g: any = globalThis, p: "webos" | "tizen" | "web" | string = detectPlatform(g)): TvDeviceInfo | null {
  try {
    if (p === "tizen") {
      const pi = g.webapis?.productinfo;
      const info: TvDeviceInfo = { platform: "tizen" };
      const call = <T>(f?: () => T): T | undefined => {
        try { return f ? f.call(pi) : undefined; } catch { return undefined; }
      };
      info.model = call(pi?.getRealModel) ?? call(pi?.getModel);
      info.firmware = call(pi?.getFirmware);
      info.uhd = call(pi?.isUdPanelSupported);
      info.uhd8k = call(pi?.is8KPanelSupported);
      try { info.hdr = g.webapis?.avinfo?.isHdrTvSupport?.(); } catch { /* unsupported */ }
      return info;
    }
    if (p === "webos") {
      let d: any;
      try { d = g.PalmSystem?.deviceInfo ? JSON.parse(g.PalmSystem.deviceInfo) : undefined; } catch { /* ignore */ }
      if (!d) return { platform: "webos" };
      return {
        platform: "webos",
        model: d.modelName,
        firmware: d.version ?? d.sdkVersion,
        uhd: typeof d.screenWidth === "number" ? d.screenWidth >= 3840 : undefined,
        uhd8k: typeof d.screenWidth === "number" ? d.screenWidth >= 7680 : undefined,
      };
    }
  } catch {
    /* ignore */
  }
  return null;
}

const SRV = (name: string, hevcHdr: string[], srt: boolean, dv: boolean): ServerDeviceProfile => ({
  name,
  player: "native",
  containers: name === "webos" ? ["mp4", "mkv", "ts", "mov"] : ["mp4", "mkv", "ts"],
  video: [
    { codec: "h264", max_profile: "high", max_level: 51, max_bit_depth: 8, hdr: [] },
    { codec: "hevc", max_profile: "main 10", max_level: 153, max_bit_depth: 10, hdr: hevcHdr.concat(dv ? ["dv"] : []) },
    { codec: "vp9", max_profile: null, max_level: null, max_bit_depth: 10, hdr: hevcHdr.slice(0, 1) },
    { codec: "av1", max_profile: null, max_level: null, max_bit_depth: 10, hdr: hevcHdr.slice(0, 1) },
  ],
  audio: ["aac:6", "ac3:6", "eac3:8", "mp3:2", "opus:2", "flac:6"].map((s) => {
    const [codec, ch] = s.split(":");
    return { codec: codec!, max_channels: Number(ch) };
  }),
  subtitles: { text: srt ? ["vtt", "srt"] : ["vtt"], image: [] },
  max_width: 3840,
  max_height: 2160,
});

/**
 * Presets for TVs; when the panel is known to be limited (HD-only, no HDR) we send a custom profile
 * that caps resolution / drops HDR so the server tone-maps / downscales instead of direct-playing.
 */
export function deviceProfileFromInfo(info: TvDeviceInfo | null, p: "webos" | "tizen"): DeviceProfileRequest {
  if (!info || (info.uhd !== false && info.hdr !== false)) return { preset: p };
  const prof = p === "webos" ? SRV("webos", ["hdr10", "hlg"], false, true) : SRV("tizen", ["hdr10", "hlg"], true, false);
  if (info.uhd === false) {
    prof.max_width = 1920;
    prof.max_height = 1080;
  }
  if (info.hdr === false) {
    prof.video.forEach((v) => (v.hdr = []));
  }
  return { profile: { ...prof, name: `${p}-custom` } };
}

/** `{preset:"webos"|"tizen"}` on TVs (refined to a custom profile if the panel is limited), else the probed web profile. */
export function detectDeviceProfile(): DeviceProfileRequest | DeviceProfile {
  if (platform === "webos" || platform === "tizen") {
    return deviceProfileFromInfo(detectTvInfo(globalThis, platform), platform);
  }
  return detectWebProfile();
}

/** Choose the player for the current platform: AVPlay (Tizen), native <video> (webOS), hls.js otherwise. */
export function createPlayerAdapter(el: HTMLVideoElement): PlayerAdapter {
  if (platform === "tizen" && typeof webapis !== "undefined" && webapis?.avplay) return new AvPlayAdapter(el);
  if (platform === "webos") return new WebOsAdapter(el);
  return new HlsJsAdapter(el);
}

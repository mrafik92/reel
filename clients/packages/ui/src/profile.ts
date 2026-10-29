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

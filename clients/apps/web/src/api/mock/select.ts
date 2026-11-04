import type { MediaFile } from "@reel/ui";
import { DEFAULT_PREFS, type PlaybackPrefs, type Selected } from "../prefs";
import { previewSelection } from "../../lib/tracks";

/** Mock of the server's automatic track choice (`selected`), from the mock-mode preferences. */
export function mockSelected(f: MediaFile, audioReq: number | null | undefined, subReq: number | null | undefined, forCast = false): Selected {
  let p: PlaybackPrefs = DEFAULT_PREFS;
  try {
    const v = localStorage.getItem("reel.mock.prefs");
    if (v) p = { ...DEFAULT_PREFS, ...(JSON.parse(v) as Partial<PlaybackPrefs>) };
  } catch {
    /* defaults */
  }
  const auto = previewSelection(f.audio, f.subtitles, p);
  const why: string[] = [];
  const audio = audioReq ?? auto.audio;
  why.push(audioReq != null ? "audio chosen by the client" : p.audio_languages[0] === "original" ? "audio: original language" : "audio: preferred language");
  let sub: number | null;
  if (subReq != null && subReq >= 0) {
    sub = subReq;
    why.push("subtitle chosen by the client");
  } else if (subReq === -1) {
    sub = null;
    why.push("subtitles turned off by the client");
  } else {
    sub = auto.sub;
    why.push(sub != null ? `subtitles: ${p.subtitle_mode === "auto" ? "audio isn't in one of your languages" : "your subtitle languages"}` : "subtitles: not needed");
  }
  if (forCast) why.push("for a cast receiver");
  return { audio_index: audio, subtitle_index: sub, reason: why.join("; ") };
}

/** Mock of the server's quality ladder (`decide_quality`): original ≈ file bitrate, 8/4/2 Mbps rungs. */
export function mockQuality(f: MediaFile, body: { quality?: string; max_bitrate_kbps?: number }) {
  const src = f.duration_s ? Math.round((f.size * 8) / f.duration_s / 1000) : 9000;
  const h = f.video?.height ?? 1080;
  const rungs = [
    { id: "original", label: `Original (${(src / 1000).toFixed(1)} Mbps)`, bitrate_kbps: src, available: true },
    { id: "1080p", label: "1080p (8.3 Mbps)", bitrate_kbps: 8256, available: h >= 1000 },
    { id: "720p", label: "720p (4.2 Mbps)", bitrate_kbps: 4160, available: h >= 700 },
    { id: "480p", label: "480p (2.1 Mbps)", bitrate_kbps: 2128, available: true },
  ];
  const cap = body.max_bitrate_kbps && body.max_bitrate_kbps > 0 ? body.max_bitrate_kbps : Infinity;
  const want = body.quality && body.quality !== "auto" ? rungs.find((r) => r.id === body.quality && r.available) : undefined;
  const pick = want ?? rungs.find((r) => r.available && r.bitrate_kbps <= cap) ?? rungs[rungs.length - 1]!;
  return {
    quality: pick.id,
    bitrate_kbps: pick.bitrate_kbps,
    quality_options: [{ id: "auto", label: "Auto", bitrate_kbps: pick.bitrate_kbps, available: true }, ...rungs],
  };
}

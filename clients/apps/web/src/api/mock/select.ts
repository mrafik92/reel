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

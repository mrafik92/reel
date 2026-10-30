/**
 * Per-user playback preferences — `GET/PUT /api/v1/me/preferences`.
 *
 * Hand-written until the server's OpenAPI includes them. The server uses these to pick
 * audio + subtitles automatically (`selected` on decide / vod play); the Settings → Playback
 * page edits them and the player offers quick saves ("Always use English subtitles?").
 */
import { MOCK } from "../lib/mode";
import { http, request } from "./http";
import type { DecideResponse } from "./types";

export type SubtitleMode = "off" | "forced_only" | "auto" | "always";

export interface PlaybackPrefs {
  /** Ordered; ISO 639-1 codes, or `"original"` = the title's original language. */
  audio_languages: string[];
  subtitle_mode: SubtitleMode;
  /** Ordered ISO 639-1 codes. */
  subtitle_languages: string[];
  prefer_sdh: boolean;
  /** Allow picture subtitles (PGS / VobSub) that have to be burned in (transcode). */
  burn_in_image_subs: boolean;
}

/** Same defaults as the server (`PlaybackPrefs::default`). */
export const DEFAULT_PREFS: PlaybackPrefs = {
  audio_languages: ["original"],
  subtitle_mode: "auto",
  subtitle_languages: ["en"],
  prefer_sdh: false,
  burn_in_image_subs: false,
};

/** The server's automatic track choice (or the client's explicit one) and why. */
export interface Selected {
  audio_index: number | null;
  /** `null` = no subtitles. */
  subtitle_index: number | null;
  /** Human readable, `; `-separated ("audio: original language (English); subtitles: …"). */
  reason: string;
}

/** `selected` until the generated types include it (older servers omit it). */
export function selectedOf(d: DecideResponse | undefined): Selected | undefined {
  const s = (d as (DecideResponse & { selected?: Selected | null }) | undefined)?.selected;
  return s ?? undefined;
}

export interface PrefsApi {
  get(): Promise<PlaybackPrefs>;
  put(p: PlaybackPrefs): Promise<PlaybackPrefs>;
}

const httpPrefs: PrefsApi = {
  get: () => http.get("/me/preferences"),
  // `http` has no `put` helper; keep the one-off here rather than touching the shared client.
  put: (p) => request("PUT", "/me/preferences", { body: p }),
};

/** Mock mode: preferences live in localStorage so the Settings page round-trips. */
const MOCK_KEY = "reel.mock.prefs";
const mockPrefs: PrefsApi = {
  async get() {
    await new Promise((r) => setTimeout(r, 120));
    try {
      const v = localStorage.getItem(MOCK_KEY);
      return v ? { ...DEFAULT_PREFS, ...(JSON.parse(v) as Partial<PlaybackPrefs>) } : { ...DEFAULT_PREFS };
    } catch {
      return { ...DEFAULT_PREFS };
    }
  },
  async put(p) {
    await new Promise((r) => setTimeout(r, 160));
    const clean: PlaybackPrefs = {
      ...p,
      audio_languages: p.audio_languages.length ? dedupe(p.audio_languages) : ["original"],
      subtitle_languages: dedupe(p.subtitle_languages.filter((l) => l !== "original")),
    };
    try {
      localStorage.setItem(MOCK_KEY, JSON.stringify(clean));
    } catch {
      /* storage disabled */
    }
    return clean;
  },
};

function dedupe(l: string[]): string[] {
  const out: string[] = [];
  for (const x of l) if (out.indexOf(x) < 0) out.push(x);
  return out;
}

export const prefsApi: PrefsApi = MOCK ? mockPrefs : httpPrefs;

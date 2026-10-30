import { isSdh, languageKey, type Id } from "@reel/ui";
import { getTrackPrefs } from "./playback";
import { currentUser } from "./auth";

/**
 * Subtitle defaults: explicit choice (URL) → this item's last choice → the
 * user's last subtitle language → off. The language memory is per user and
 * per device (localStorage), so "English SDH, please" follows you around.
 */

interface LangPref {
  /** `languageKey()` of the last picked track, or "off". */
  lang: string;
  sdh?: boolean;
}

function key(): string {
  return `reel.sublang.${currentUser()?.id ?? "anon"}`;
}

export function getSubLanguagePref(): LangPref | undefined {
  try {
    const v = localStorage.getItem(key());
    return v ? (JSON.parse(v) as LangPref) : undefined;
  } catch {
    return undefined;
  }
}

/** Remember the language of a subtitle pick (or "off") as the default for next time. */
export function rememberSubtitle(s: { language?: string | null; title?: string | null } | null): void {
  try {
    const v: LangPref = s ? { lang: languageKey(s.language) || "off", sdh: isSdh(s) } : { lang: "off" };
    localStorage.setItem(key(), JSON.stringify(v));
  } catch {
    /* storage disabled */
  }
}

interface SubLike {
  index: number;
  language?: string | null;
  title?: string | null;
  forced?: boolean;
}

/** Pick the subtitle stream index to start with (-1 = off). */
export function initialSubtitle(itemId: Id, subs: SubLike[], explicit?: string | null): number {
  if (explicit != null && explicit !== "") {
    const n = Number(explicit);
    if (n === -1 || subs.some((s) => s.index === n)) return n;
  }
  const item = getTrackPrefs(itemId).sub;
  if (item != null && (item === -1 || subs.some((s) => s.index === item))) return item;
  const pref = getSubLanguagePref();
  if (!pref || pref.lang === "off") return -1;
  const same = subs.filter((s) => languageKey(s.language) === pref.lang && !s.forced);
  if (!same.length) return -1;
  // Prefer the same SDH-ness as last time, else the first in that language.
  return (same.find((s) => isSdh(s) === !!pref.sdh) ?? same[0]!).index;
}

/** Audio stream index to start with (null = let the server choose the default). */
export function initialAudio(itemId: Id, audio: { index: number }[], explicit?: string | null): number | null {
  if (explicit != null && explicit !== "" && audio.some((a) => a.index === Number(explicit))) return Number(explicit);
  const item = getTrackPrefs(itemId).audio;
  return item != null && audio.some((a) => a.index === item) ? item : null;
}

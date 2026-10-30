import { createSignal } from "solid-js";
import { ApiError } from "../api/http";
import { DEFAULT_PREFS, prefsApi, type PlaybackPrefs } from "../api/prefs";
import { onAuthChange } from "./auth";

/**
 * The signed-in user's playback preferences, loaded once and shared by the item page,
 * the player (quick saves) and Settings → Playback.
 */
const [prefs, setPrefs] = createSignal<PlaybackPrefs>();
/** `false` when the server doesn't know `/me/preferences` yet (older build). */
const [supported, setSupported] = createSignal<boolean>();
let loading: Promise<PlaybackPrefs> | undefined;

export { prefs as playbackPrefs, supported as prefsSupported };

onAuthChange(() => {
  loading = undefined;
  setPrefs(undefined);
  setSupported(undefined);
});

/** Load (once) — never throws: falls back to the server defaults. */
export function loadPrefs(force = false): Promise<PlaybackPrefs> {
  if (!loading || force) {
    loading = prefsApi.get().then(
      (p) => {
        const v = { ...DEFAULT_PREFS, ...p };
        setPrefs(v);
        setSupported(true);
        return v;
      },
      (e) => {
        if (e instanceof ApiError && (e.status === 404 || e.status === 405)) setSupported(false);
        loading = undefined;
        const v = prefs() ?? { ...DEFAULT_PREFS };
        if (!prefs()) setPrefs(v);
        return v;
      },
    );
  }
  return loading;
}

/** Save the whole object; the server normalises codes (`eng` → `en`) and returns the stored copy. */
export async function savePrefs(next: PlaybackPrefs): Promise<PlaybackPrefs> {
  const before = prefs();
  setPrefs(next);
  try {
    const saved = { ...DEFAULT_PREFS, ...(await prefsApi.put(next)) };
    setPrefs(saved);
    setSupported(true);
    loading = Promise.resolve(saved);
    return saved;
  } catch (e) {
    setPrefs(before);
    throw e;
  }
}

/** Merge a change into the current preferences and save it. */
export async function updatePrefs(patch: Partial<PlaybackPrefs>): Promise<PlaybackPrefs> {
  const cur = prefs() ?? (await loadPrefs());
  return savePrefs({ ...cur, ...patch });
}

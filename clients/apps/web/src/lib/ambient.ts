import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";

/** The smallest width the image API serves. Stretched, it blurs by itself: no CSS filter on TVs. */
export const AMBIENT_W = 92;

/**
 * The URL for an ambient backdrop. A real image is requested at `AMBIENT_W` by the caller. Mock art
 * is SVG, which stays sharp at any size, so it is drawn to a thumbnail first to look the same.
 */
export function useAmbientArt(src: Accessor<string | null | undefined>): Accessor<string | null | undefined> {
  const [url, setUrl] = createSignal<string | null | undefined>();
  createEffect(() => {
    const s = src();
    if (!s || s.indexOf("data:image/svg") !== 0) {
      setUrl(s);
      return;
    }
    let live = true;
    onCleanup(() => (live = false));
    const img = new Image();
    img.onload = () => {
      if (!live) return;
      const c = document.createElement("canvas");
      c.width = AMBIENT_W;
      c.height = Math.round((AMBIENT_W * 9) / 16);
      const g = c.getContext("2d");
      if (!g) return setUrl(s);
      g.drawImage(img, 0, 0, c.width, c.height);
      setUrl(c.toDataURL("image/png"));
    };
    img.onerror = () => live && setUrl(null);
    img.src = s;
  });
  return url;
}

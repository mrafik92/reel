import { createEffect, createSignal, For, onCleanup, type Accessor, type JSX, type Setter } from "solid-js";
import { mediaUrl } from "../format";

interface Layer {
  url: string;
  id: number;
  shown: Accessor<boolean>;
  setShown: Setter<boolean>;
  loaded?: boolean;
}

/**
 * Full-bleed backdrop that cross-fades between images. Only opacity animates,
 * which stays on the compositor on low-power TV SoCs.
 */
export function Backdrop(props: { src?: string | null; width?: number; class?: string }): JSX.Element {
  const [layers, setLayers] = createSignal<Layer[]>([]);
  let n = 0;
  const failed = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => timer && clearTimeout(timer));

  createEffect(() => {
    const u = mediaUrl(props.src, props.width ?? 1280);
    const ls = layers();
    const top = ls[ls.length - 1];
    if (!u || failed.has(u)) {
      // No art for this title: don't keep showing the previous one.
      ls.forEach((l) => l.setShown(false));
      return;
    }
    if (top && top.url === u) {
      if (!top.shown() && top.loaded) top.setShown(true);
      return;
    }
    const [shown, setShown] = createSignal(false);
    // Keep at most the currently visible layer underneath the incoming one.
    const keep = ls.filter((l) => l.shown()).slice(-1);
    setLayers(keep.concat({ url: u, id: ++n, shown, setShown }));
  });

  return (
    <div class={"backdrop" + (props.class ? " " + props.class : "")} aria-hidden="true">
      <For each={layers()}>
        {(l) => (
          <img
            class="backdrop__img"
            classList={{ "is-shown": l.shown() }}
            src={l.url}
            alt=""
            decoding="async"
            onLoad={() => {
              l.loaded = true;
              l.setShown(true);
              if (timer) clearTimeout(timer);
              timer = setTimeout(() => setLayers((ls) => ls.filter((x) => x.id >= l.id)), 700);
            }}
            onError={() => {
              failed.add(l.url);
              // Missing art (404): fade whatever was showing out to the plain background.
              layers().forEach((x) => x.id <= l.id && x.setShown(false));
              if (timer) clearTimeout(timer);
              timer = setTimeout(() => setLayers((ls) => ls.filter((x) => x.id > l.id)), 700);
            }}
          />
        )}
      </For>
    </div>
  );
}

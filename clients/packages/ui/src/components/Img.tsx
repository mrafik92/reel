import { createMemo, createSignal, on, createEffect, onCleanup, onMount, Show, type JSX } from "solid-js";
import { mediaUrl } from "../format";

let io: IntersectionObserver | undefined;
const pending = new WeakMap<Element, () => void>();

function observer(): IntersectionObserver | undefined {
  if (typeof IntersectionObserver === "undefined") return undefined;
  if (!io) {
    io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting || e.intersectionRatio > 0) {
            const cb = pending.get(e.target);
            if (cb) {
              pending.delete(e.target);
              io!.unobserve(e.target);
              cb();
            }
          }
        }
      },
      // Generous margins: horizontal rows prefetch ~2 cards ahead.
      { rootMargin: "600px 900px" },
    );
  }
  return io;
}

export interface ImgProps {
  src?: string | null;
  /** Requested width (server resizes via `?w=`). */
  width?: number;
  alt?: string;
  class?: string;
  /** Load immediately (hero/backdrop). */
  eager?: boolean;
  /** Shown when there's no image or it fails. */
  fallback?: JSX.Element;
}

/** Lazy, fade-in image that degrades to a typographic fallback. */
export function Img(props: ImgProps): JSX.Element {
  const [visible, setVisible] = createSignal(!!props.eager);
  const [loaded, setLoaded] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  // Already in the cache: show it at once. The fade is for art that arrives, not art that is there.
  const [instant, setInstant] = createSignal(false);
  let wrap!: HTMLDivElement;

  onMount(() => {
    if (visible()) return;
    const o = observer();
    if (!o) return setVisible(true);
    pending.set(wrap, () => setVisible(true));
    o.observe(wrap);
    onCleanup(() => {
      pending.delete(wrap);
      o.unobserve(wrap);
    });
  });

  // Memo: only a *different* URL resets the fade (parents often re-emit the same src).
  const url = createMemo(() => mediaUrl(props.src, props.width));
  createEffect(on(url, () => {
    setLoaded(false);
    setFailed(false);
    setInstant(false);
  }, { defer: true }));
  const cached = (img: HTMLImageElement) => {
    queueMicrotask(() => {
      if (img.complete && img.naturalWidth > 0) {
        setInstant(true);
        setLoaded(true);
      }
    });
  };

  return (
    <div ref={wrap} class={"img" + (props.class ? " " + props.class : "")} classList={{ "is-loaded": loaded(), "is-instant": instant() }}>
      <Show when={url() && !failed()} fallback={props.fallback}>
        <Show when={visible()}>
          <img
            ref={cached}
            src={url()}
            alt={props.alt ?? ""}
            decoding="async"
            draggable={false}
            onLoad={() => setLoaded(true)}
            onError={() => setFailed(true)}
          />
        </Show>
      </Show>
    </div>
  );
}

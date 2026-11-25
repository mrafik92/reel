import { onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, Icon, onBack } from "@reel/ui";
import "../styles/trailer.css";

/** YouTube trailer in a full-screen overlay (Back / Esc closes). */
export function TrailerModal(props: { youtubeId: string; title: string; onClose: () => void }): JSX.Element {
  let close!: HTMLButtonElement;
  let returnTo: HTMLElement | null = null;
  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => focusElement(close, { smooth: false }));
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });
  return (
    <Portal>
      <div class="trailer" role="dialog" aria-modal="true" aria-label={`Trailer: ${props.title}`} data-nav-trap data-nav-fixed>
        <div class="trailer__scrim" onClick={() => props.onClose()} />
        <div class="trailer__frame">
          <iframe
            src={`https://www.youtube-nocookie.com/embed/${encodeURIComponent(props.youtubeId)}?autoplay=1&rel=0&modestbranding=1`}
            title={`Trailer: ${props.title}`}
            allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
            allowfullscreen
          />
        </div>
        <button ref={close} type="button" class="btn trailer__close" onClick={() => props.onClose()}>
          <Icon name="x" /> <span>Close trailer</span>
        </button>
      </div>
    </Portal>
  );
}

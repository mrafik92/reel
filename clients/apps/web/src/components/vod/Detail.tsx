import { createResource, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, Icon, onBack } from "@reel/ui";
import { vod, type VodId, type VodKind } from "../../api/vod";
import { CategoryLabel } from "./Categories";
import { TitleSheet } from "../TitleSheet";
import "../../styles/vod.css";

/** "IPTV · Film · AR | Egyptian Movies" — where this title comes from. */
export function VodEyebrow(props: { kind: VodKind; categoryId: VodId }): JSX.Element {
  const [cats] = createResource(() => props.kind, (k) => vod.categories(k).catch(() => []));
  const cat = () => (cats() ?? []).find((c) => String(c.id) === String(props.categoryId));
  return (
    <p class="eyebrow veyebrow">
      <span class="vbadge vbadge--inline"><Icon name="live" /> IPTV</span>
      <span>{props.kind === "movie" ? "Film" : "Series"}</span>
      <Show when={cat()}>
        <span class="veyebrow__cat"><CategoryLabel name={cat()!.name} /></span>
      </Show>
    </p>
  );
}

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

/** Small print under the actions: what IPTV playback means. */
export function VodFineprint(props: { local?: boolean }): JSX.Element {
  return (
    <p class="vfine">
      <Icon name="live" />
      <span>
        {props.local
          ? "Your library copy plays from your own server. The IPTV copy streams from your provider, one screen at a time."
          : "Streams from your IPTV provider — one screen at a time across your household."}
      </span>
    </p>
  );
}

export function VodSkeleton(): JSX.Element {
  return (
    <TitleSheet head={
      <div aria-busy="true" aria-label="Loading">
        <span class="skel skel--title" />
        <span class="skel skel--text" style={{ width: "14rem", height: "3.25rem" }} />
      </div>
    }>
      <section class="tinfo">
        <div class="tinfo__main">
          <span class="skel skel--text" style={{ width: "60%" }} />
          <span class="skel skel--text" style={{ width: "90%" }} />
          <span class="skel skel--text" style={{ width: "80%" }} />
        </div>
      </section>
    </TitleSheet>
  );
}

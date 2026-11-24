import { For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, focusFirst, Icon, keepFocusOnOverlayMove, onBack, overlayMount, SheetClose } from "@reel/ui";

export interface TrackOption {
  value: number;
  title: string;
  detail?: string;
}

/**
 * Audio and subtitles in one menu (#293): two columns in the player's compact panel. A pick closes
 * the menu first and then applies, like `Sheet`, because a new track can restart the stream.
 */
export function TracksMenu(props: {
  audio: TrackOption[];
  audioSelected: number;
  onAudio: (v: number) => void;
  subs: TrackOption[];
  subSelected: number;
  onSub: (v: number) => void;
  /** Line under the columns, e.g. why subtitles are on. */
  note?: string;
  footer?: JSX.Element;
  onClose: () => void;
}): JSX.Element {
  let panel!: HTMLDivElement;
  let returnTo: HTMLElement | null = null;
  let picked = false;
  const pick = (apply: () => void) => {
    if (picked) return;
    picked = true;
    props.onClose();
    setTimeout(apply, 0);
  };
  // Subtitles change far more often than audio: open on the current subtitle choice.
  const focusCurrent = (p: HTMLElement) => {
    const sel =
      p.querySelector<HTMLElement>('.sheet__col--subs [aria-checked="true"]') ?? p.querySelector<HTMLElement>('[aria-checked="true"]');
    if (sel) focusElement(sel, { smooth: false });
    else focusFirst(p);
  };
  keepFocusOnOverlayMove(() => panel, focusCurrent);

  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    focusCurrent(panel);
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });

  const column = (name: string, cls: string, options: () => TrackOption[], selected: () => number, apply: (v: number) => void) => (
    <section class={`sheet__col ${cls}`}>
      <h3 class="sheet__colhead">{name}</h3>
      <div class="sheet__list" role="radiogroup" aria-label={name} data-nav-scroll="y">
        <For each={options()}>
          {(o) => (
            <button
              type="button"
              role="radio"
              class="sheet__opt"
              aria-checked={o.value === selected() ? "true" : "false"}
              onClick={() => pick(() => apply(o.value))}
            >
              <span class="sheet__mark">
                <Show when={o.value === selected()}><Icon name="check" /></Show>
              </span>
              <span class="sheet__text">
                <span class="sheet__title">{o.title}</span>
                <Show when={o.detail}>
                  <span class="sheet__detail">{o.detail}</span>
                </Show>
              </span>
            </button>
          )}
        </For>
      </div>
    </section>
  );

  return (
    <Portal mount={overlayMount()}>
      <div
        class="sheet sheet--pop sheet--duo"
        data-nav-trap
        data-nav-fixed
        onKeyDown={(e) => {
          if (e.key === "Escape" || e.key === "Esc") {
            e.preventDefault();
            e.stopPropagation();
            props.onClose();
          }
        }}
      >
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel" ref={panel} role="dialog" aria-modal="true" aria-label="Audio and subtitles">
          <SheetClose onClose={props.onClose} />
          <div class="sheet__cols">
            {column("Audio", "sheet__col--audio", () => props.audio, () => props.audioSelected, props.onAudio)}
            {column("Subtitles", "sheet__col--subs", () => props.subs, () => props.subSelected, props.onSub)}
          </div>
          <Show when={props.note}>
            <p class="sheet__desc">{props.note}</p>
          </Show>
          <Show when={props.footer}>
            <footer class="sheet__foot">{props.footer}</footer>
          </Show>
        </div>
      </div>
    </Portal>
  );
}

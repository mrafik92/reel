import { For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, focusFirst, onBack } from "../navigation";
import { Icon } from "./Icon";

export interface SheetOption<V> {
  value: V;
  title: string;
  detail?: string;
}

export interface SheetProps<V> {
  title: string;
  options: SheetOption<V>[];
  selected: V;
  onSelect: (v: V) => void;
  onClose: () => void;
}

/**
 * Side sheet picker (audio, subtitles, versions). Traps D-pad focus and closes
 * on Back. Solid, opaque surface — no backdrop blur on TV hardware.
 */
export function Sheet<V>(props: SheetProps<V>): JSX.Element {
  let panel!: HTMLDivElement;
  let returnTo: HTMLElement | null = null;

  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    const sel = panel.querySelector<HTMLElement>('[aria-checked="true"]');
    if (sel) focusElement(sel, { smooth: false });
    else focusFirst(panel);
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
      <div class="sheet" data-nav-trap data-nav-fixed>
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel" ref={panel} role="dialog" aria-modal="true" aria-label={props.title}>
          <header class="sheet__head">
            <h2>{props.title}</h2>
          </header>
          <div class="sheet__list" role="radiogroup" data-nav-scroll="y">
            <For each={props.options}>
              {(o) => (
                <button
                  type="button"
                  role="radio"
                  class="sheet__opt"
                  aria-checked={o.value === props.selected ? "true" : "false"}
                  onClick={() => {
                    props.onSelect(o.value);
                    props.onClose();
                  }}
                >
                  <span class="sheet__mark">
                    <Show when={o.value === props.selected}>
                      <Icon name="check" />
                    </Show>
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
          <footer class="sheet__foot">
            <span class="keycap">Back</span> to close
          </footer>
        </div>
      </div>
    </Portal>
  );
}

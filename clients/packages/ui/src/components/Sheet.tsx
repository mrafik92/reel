import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, focusFirst, onBack } from "../navigation";
import { Icon, type IconName } from "./Icon";

export interface SheetOption<V> {
  value: V;
  title: string;
  detail?: string;
  /** Leading icon instead of the check column (target lists, e.g. "Play on…"). */
  icon?: IconName;
  /** Small pill after the title, e.g. "Auto". */
  tag?: string;
  disabled?: boolean;
}

export interface SheetProps<V> {
  title: string;
  options: SheetOption<V>[];
  selected: V;
  onSelect: (v: V) => void;
  onClose: () => void;
  /** Line under the title. */
  description?: JSX.Element;
  /** Extra content after the options (status rows, links). */
  children?: JSX.Element;
  /** Replaces the "Back to close" hint. */
  footer?: JSX.Element;
}

/**
 * Side sheet picker (audio, subtitles, versions). Traps D-pad focus and closes
 * on Back. Solid, opaque surface — no backdrop blur on TV hardware.
 */
export function Sheet<V>(props: SheetProps<V>): JSX.Element {
  let panel!: HTMLDivElement;
  let returnTo: HTMLElement | null = null;
  // Optimistic: the tick moves the moment you pick, whatever the parent does next.
  const [chosen, setChosen] = createSignal<V>(props.selected);
  let picked = false;
  const pick = (v: V) => {
    if (picked) return;
    picked = true;
    setChosen(() => v);
    // Close first, then apply: a selection that restarts playback (new session,
    // re-render of the option list, or an error) can never leave the sheet stuck.
    props.onClose();
    setTimeout(() => props.onSelect(v), 0);
  };

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
      <div
        class="sheet"
        data-nav-trap
        data-nav-fixed
        onKeyDown={(e) => {
          // Escape closes the sheet only — never falls through to page "Back".
          if (e.key === "Escape" || e.key === "Esc") {
            e.preventDefault();
            e.stopPropagation();
            props.onClose();
          }
        }}
      >
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel" ref={panel} role="dialog" aria-modal="true" aria-label={props.title}>
          <header class="sheet__head">
            <h2>{props.title}</h2>
            <Show when={props.description}>
              <p class="sheet__desc">{props.description}</p>
            </Show>
          </header>
          <div class="sheet__list" role="radiogroup" data-nav-scroll="y">
            <For each={props.options}>
              {(o) => (
                <button
                  type="button"
                  role="radio"
                  class="sheet__opt"
                  classList={{ "sheet__opt--icon": !!o.icon }}
                  aria-checked={o.value === chosen() ? "true" : "false"}
                  disabled={o.disabled}
                  onClick={() => pick(o.value)}
                >
                  <span class="sheet__mark">
                    <Show when={o.icon} fallback={<Show when={o.value === chosen()}><Icon name="check" /></Show>}>
                      <Icon name={o.icon!} />
                    </Show>
                  </span>
                  <span class="sheet__text">
                    <span class="sheet__title">
                      {o.title}
                      <Show when={o.tag}><small class="sheet__tag">{o.tag}</small></Show>
                    </span>
                    <Show when={o.detail}>
                      <span class="sheet__detail">{o.detail}</span>
                    </Show>
                  </span>
                </button>
              )}
            </For>
            {props.children}
          </div>
          <footer class="sheet__foot">
            <Show when={props.footer} fallback={<><span class="keycap">Back</span> to close</>}>
              {props.footer}
            </Show>
          </footer>
        </div>
      </div>
    </Portal>
  );
}

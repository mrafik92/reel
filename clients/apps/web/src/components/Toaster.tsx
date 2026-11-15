import { For, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { Icon, overlayMount } from "@reel/ui";
import { dismiss, toasts } from "../lib/toast";

/** Toasts mount inside the full-screen element (the player) when there is one, so errors
 * raised during full-screen playback are visible (#94). */
export function Toaster(): JSX.Element {
  return (
    <Portal mount={overlayMount()}>
      <div class="toaster" role="status" aria-live="polite" data-nav-skip>
        <For each={toasts()}>
          {(t) => (
            <div class={`toast toast--${t.tone}`} onClick={() => dismiss(t.id)}>
              <Icon name={t.tone === "error" ? "alert" : t.tone === "success" ? "check" : "info"} />
              <span>{t.text}</span>
            </div>
          )}
        </For>
      </div>
    </Portal>
  );
}

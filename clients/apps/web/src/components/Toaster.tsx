import { For, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { dismiss, toasts } from "../lib/toast";

export function Toaster(): JSX.Element {
  return (
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
  );
}

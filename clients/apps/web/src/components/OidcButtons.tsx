import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { api } from "../api";
import { oidcSupported, startOidc } from "../lib/oidc";

/** "Continue with Google" and friends; nothing at all when no provider is configured. */
export function OidcButtons(props: { purpose: "login" | "invite"; invite?: string; next?: string; onError: (m: string) => void }): JSX.Element {
  const [providers] = createResource(() => (oidcSupported() ? api.oidcProviders().catch(() => []) : []));
  const [busy, setBusy] = createSignal(false);
  const go = async (p: { slug: string; name: string }) => {
    if (busy()) return;
    setBusy(true);
    try {
      await startOidc(p, props.purpose, { invite: props.invite, next: props.next });
    } catch (e) {
      props.onError(e instanceof Error ? e.message : `Couldn't reach ${p.name}.`);
      setBusy(false);
    }
  };
  return (
    <Show when={providers()?.length}>
      <div class="auth__methods auth__methods--stack">
        <For each={providers()}>
          {(p) => (
            <button type="button" class="btn btn--block" aria-disabled={busy() ? "true" : undefined} onClick={() => void go(p)}>
              <span>Continue with {p.name}</span>
            </button>
          )}
        </For>
      </div>
    </Show>
  );
}

import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api } from "../../api";
import { ConfirmButton, Panel, Select, Toggle } from "../../components/AdminKit";
import { Field, PasswordField } from "../../components/Form";
import { toast } from "../../lib/toast";

const PRESETS = [
  { value: "google", label: "Google" },
  { value: "microsoft", label: "Microsoft (personal accounts)" },
  { value: "custom", label: "Other OpenID Connect provider" },
];

/** Settings → Sign-in providers (#149). A secret can be replaced but never read back. */
export default function OidcProviders(): JSX.Element {
  const [list, { refetch }] = createResource(() => api.adminOidcProviders());
  const [preset, setPreset] = createSignal("google");
  const [name, setName] = createSignal("");
  const [issuer, setIssuer] = createSignal("");
  const [clientId, setClientId] = createSignal("");
  const [secret, setSecret] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const add = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!clientId().trim() || !secret()) return void toast("Enter the client ID and secret.", "error");
    setBusy(true);
    try {
      await api.adminCreateOidcProvider({
        preset: preset(),
        name: preset() === "custom" ? name().trim() : undefined,
        issuer: preset() === "custom" ? issuer().trim() : undefined,
        client_id: clientId().trim(),
        client_secret: secret(),
      });
      setClientId("");
      setSecret("");
      setName("");
      setIssuer("");
      await refetch();
      toast("Provider added", "success");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Couldn't add the provider.", "error", 6000);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="account">
      <Panel title="Sign-in providers" icon="key">
        <p class="state__body">People link a provider to their existing account in Settings → Account. A provider login never creates an account by itself.</p>
        <ul class="list">
          <For each={list() ?? []}>
            {(p) => (
              <li class="list__row list__row--stack">
                <div class="list__line">
                  <div class="list__main">
                    <strong>{p.name}</strong>
                    <span class="list__sub">{p.issuer}</span>
                    <Show when={p.redirect_uri}><span class="list__sub">Redirect URI: <code>{p.redirect_uri}</code></span></Show>
                  </div>
                  <div class="list__actions">
                    <ConfirmButton label="Remove" confirm="Remove it and everyone's link to it?" onConfirm={async () => {
                      await api.adminDeleteOidcProvider(p.id);
                      void refetch();
                    }} />
                  </div>
                </div>
                <Toggle label="Enabled" checked={p.enabled} onChange={async (enabled) => {
                  try {
                    await api.adminPatchOidcProvider(p.id, { enabled });
                  } catch (err) {
                    toast(err instanceof Error ? err.message : "Couldn't change it.", "error");
                  }
                  void refetch();
                }} />
              </li>
            )}
          </For>
        </ul>
      </Panel>
      <Panel title="Add a provider" icon="plus">
        <form onSubmit={add} novalidate>
          <Select label="Provider" value={preset()} options={PRESETS} onChange={(e) => setPreset(e.currentTarget.value)} />
          <Show when={preset() === "custom"}>
            <Field label="Name" value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={64} />
            <Field label="Issuer URL" placeholder="https://id.example.com" value={issuer()} onInput={(e) => setIssuer(e.currentTarget.value)} inputmode="url" autocapitalize="off" spellcheck={false} />
          </Show>
          <Field label="Client ID" value={clientId()} onInput={(e) => setClientId(e.currentTarget.value)} autocomplete="off" autocapitalize="off" spellcheck={false} />
          <PasswordField label="Client secret" value={secret()} onInput={(e) => setSecret(e.currentTarget.value)} autocomplete="new-password" />
          <button type="submit" class="btn btn--primary btn--sm" aria-disabled={busy() ? "true" : undefined}><Icon name="plus" /> <span>Add provider</span></button>
        </form>
      </Panel>
    </div>
  );
}

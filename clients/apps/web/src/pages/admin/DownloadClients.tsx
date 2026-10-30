import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, type DownloadClient } from "../../api";
import { toast } from "../../lib/toast";
import { Field, PasswordField } from "../../components/Form";
import { ConfirmButton, Panel } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

export default function DownloadClients(): JSX.Element {
  const [list, { refetch }] = createResource(() => api.downloadClients());
  const [adding, setAdding] = createSignal(false);
  return (
    <Panel
      title="Download clients"
      icon="download"
      aside={
        <Show when={!adding()}>
          <button type="button" class="btn btn--primary btn--sm" onClick={() => setAdding(true)}><Icon name="plus" /> <span>Add client</span></button>
        </Show>
      }
    >
      <p class="panel__lede">Optional. With your torrent client connected, progress rings also show download speed, peers and a sharper ETA.</p>
      <Show when={adding()}>
        <AddClient onDone={(c) => { setAdding(false); if (c) void refetch(); }} />
      </Show>
      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={refetch} />}>
        <ul class="list">
          <For each={list()} fallback={<li class="panel__empty">None connected — Radarr/Sonarr progress is still shown.</li>}>
            {(c) => (
              <li class="list__row">
                <Icon name="download" class="list__icon" />
                <div class="list__main">
                  <strong>{c.kind === "qbittorrent" ? "qBittorrent" : c.kind === "transmission" ? "Transmission" : c.kind}</strong>
                  <span class="list__sub"><code>{c.url}</code>{c.username ? ` · ${c.username}` : ""}{c.password_set ? " · password set" : ""}</span>
                </div>
                <div class="list__actions">
                  <ConfirmButton label="Remove" onConfirm={async () => { await api.deleteDownloadClient(c.id); void refetch(); }} />
                </div>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

function AddClient(props: { onDone: (c?: DownloadClient) => void }): JSX.Element {
  const [kind, setKind] = createSignal<"qbittorrent" | "transmission">("qbittorrent");
  const [url, setUrl] = createSignal("");
  const [user, setUser] = createSignal("");
  const [pass, setPass] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!url().trim()) return toast("Enter the client's address", "error");
    setBusy(true);
    try {
      const c = await api.createDownloadClient({ kind: kind(), url: url().trim(), username: user().trim() || null, password: pass() || null });
      toast("Download client added", "success");
      props.onDone(c);
    } catch (err) {
      toast(err instanceof Error ? err.message : "Couldn't add the client", "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="subform" onSubmit={submit} novalidate>
      <div class="segmented" role="group" aria-label="Client">
        <button type="button" class="segmented__btn" aria-pressed={kind() === "qbittorrent"} onClick={() => setKind("qbittorrent")}>qBittorrent</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "transmission"} onClick={() => setKind("transmission")}>Transmission</button>
      </div>
      <div class="form-grid">
        <Field label="Address" placeholder={kind() === "qbittorrent" ? "http://qbittorrent:8080" : "http://transmission:9091"} value={url()} onInput={(e) => setUrl(e.currentTarget.value)} autocapitalize="off" spellcheck={false} data-autofocus />
        <Field label="Username" value={user()} onInput={(e) => setUser(e.currentTarget.value)} autocapitalize="off" spellcheck={false} />
        <PasswordField label="Password" value={pass()} onInput={(e) => setPass(e.currentTarget.value)} autocomplete="off" />
      </div>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Adding…" : "Add client"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
    </form>
  );
}

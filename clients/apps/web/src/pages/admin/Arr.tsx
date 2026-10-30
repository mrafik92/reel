import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, type ArrProfiles, type ArrServer } from "../../api";
import { toast } from "../../lib/toast";
import { Field, PasswordField } from "../../components/Form";
import { ConfirmButton, Panel, Select, Toggle } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

export default function Arr(): JSX.Element {
  const [servers, { refetch, mutate }] = createResource(() => api.arrServers());
  const [adding, setAdding] = createSignal(false);
  return (
    <Panel
      title="Radarr & Sonarr"
      icon="server"
      aside={
        <Show when={!adding()}>
          <button type="button" class="btn btn--primary btn--sm" onClick={() => setAdding(true)}><Icon name="plus" /> <span>Add server</span></button>
        </Show>
      }
    >
      <p class="panel__lede">Requests for films go to Radarr, series to Sonarr. Reel picks the default quality profile and folder below unless a request says otherwise.</p>
      <Show when={adding()}>
        <AddArr onDone={(s) => { setAdding(false); if (s) void refetch(); }} />
      </Show>
      <Show when={!servers.error} fallback={<ErrorState error={servers.error} onRetry={refetch} />}>
        <ul class="list">
          <For each={servers()} fallback={<li class="panel__empty">No servers yet — requests can't download until one is added.</li>}>
            {(s) => <ArrRow server={s} onChange={(n) => mutate((l) => (l ?? []).map((x) => (x.id === n.id ? n : x)))} onDelete={() => void refetch()} />}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

function ArrRow(props: { server: ArrServer; onChange: (s: ArrServer) => void; onDelete: () => void }): JSX.Element {
  const s = () => props.server;
  const [open, setOpen] = createSignal(false);
  const [profiles, setProfiles] = createSignal<ArrProfiles>();
  const [checking, setChecking] = createSignal(false);
  const [err, setErr] = createSignal<string>();

  const validate = async () => {
    if (checking()) return;
    setChecking(true);
    setErr(undefined);
    try {
      setProfiles(await api.arrProfiles(s().id));
      setOpen(true);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Couldn't reach the server");
    } finally {
      setChecking(false);
    }
  };
  const patch = async (b: { default_profile?: number | null; default_root?: string | null; is_4k?: boolean | null }) => {
    try {
      props.onChange(await api.updateArrServer(s().id, b));
      toast("Saved", "success", 1800);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't save", "error");
    }
  };
  const profileName = () => profiles()?.quality_profiles.find((p) => p.id === s().default_profile)?.name;

  return (
    <li class="list__row list__row--stack">
      <div class="list__line">
        <span class={`brand brand--${s().kind}`} aria-hidden="true">{s().kind === "sonarr" ? "S" : "R"}</span>
        <div class="list__main">
          <strong>{s().name} <Show when={s().is_4k}><span class="pill pill--status">4K</span></Show></strong>
          <span class="list__sub">
            <code>{s().url}</code> · key {s().api_key_hint}
            {s().default_root ? ` · ${s().default_root}` : ""}
            {profileName() ? ` · ${profileName()}` : s().default_profile ? ` · profile #${s().default_profile}` : " · no default profile"}
          </span>
          <Show when={err()}><span class="form-error"><Icon name="alert" /> {err()}</span></Show>
        </div>
        <div class="list__actions">
          <button type="button" class="btn btn--sm" aria-disabled={checking() ? "true" : undefined} onClick={() => void validate()}>
            <Icon name={profiles() ? "check" : "activity"} /> <span>{checking() ? "Testing…" : profiles() ? "Connected" : "Test & configure"}</span>
          </button>
          <ConfirmButton label="Remove" onConfirm={async () => { await api.deleteArrServer(s().id); props.onDelete(); }} />
        </div>
      </div>
      <Show when={open() && profiles()}>
        {(p) => (
          <div class="subform">
            <div class="form-grid">
              <Select
                label="Default quality profile"
                value={String(s().default_profile ?? "")}
                onChange={(e) => void patch({ default_profile: Number(e.currentTarget.value) || null })}
                options={[{ value: "", label: "Choose…" }].concat(p().quality_profiles.map((q) => ({ value: String(q.id), label: q.name })))}
              />
              <Select
                label="Default root folder"
                value={s().default_root ?? ""}
                onChange={(e) => void patch({ default_root: e.currentTarget.value || null })}
                options={[{ value: "", label: "Choose…" }].concat(
                  p().root_folders.map((r) => ({ value: r.path, label: `${r.path}${r.free_space ? ` (${(r.free_space / 1e12).toFixed(1)} TB free)` : ""}` })),
                )}
              />
            </div>
            <Toggle label="4K server" hint="Used for 4K requests when you run a separate instance" checked={s().is_4k} onChange={(v) => void patch({ is_4k: v })} />
          </div>
        )}
      </Show>
    </li>
  );
}

function AddArr(props: { onDone: (s?: ArrServer) => void }): JSX.Element {
  const [kind, setKind] = createSignal<"radarr" | "sonarr">("radarr");
  const [name, setName] = createSignal("Radarr");
  const [url, setUrl] = createSignal("");
  const [key, setKey] = createSignal("");
  const [is4k, setIs4k] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const pick = (k: "radarr" | "sonarr") => {
    if (name() === "Radarr" || name() === "Sonarr" || !name()) setName(k === "radarr" ? "Radarr" : "Sonarr");
    setKind(k);
  };
  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!url().trim() || !key().trim()) return setError("The address and API key are both needed.");
    setBusy(true);
    setError(undefined);
    try {
      const s = await api.createArrServer({ kind: kind(), name: name().trim() || kind(), url: url().trim(), api_key: key().trim(), is_4k: is4k() });
      // Validate straight away so a typo surfaces now, not on the first request.
      try {
        await api.arrProfiles(s.id);
        toast(`Connected to ${s.name}`, "success");
      } catch (err) {
        toast(`Saved, but couldn't reach ${s.name}: ${err instanceof Error ? err.message : "unknown error"}`, "error", 7000);
      }
      props.onDone(s);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Couldn't add the server");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="subform" onSubmit={submit} novalidate>
      <div class="segmented" role="group" aria-label="Server type">
        <button type="button" class="segmented__btn" aria-pressed={kind() === "radarr"} onClick={() => pick("radarr")}>Radarr · films</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "sonarr"} onClick={() => pick("sonarr")}>Sonarr · series</button>
      </div>
      <div class="form-grid">
        <Field label="Name" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <Field label="Address" placeholder={kind() === "radarr" ? "http://radarr:7878" : "http://sonarr:8989"} value={url()} onInput={(e) => setUrl(e.currentTarget.value)} autocapitalize="off" spellcheck={false} inputmode="url" data-autofocus />
        <PasswordField label="API key" hint="Settings → General in Radarr/Sonarr" value={key()} onInput={(e) => setKey(e.currentTarget.value)} autocomplete="off" />
      </div>
      <Toggle label="4K server" checked={is4k()} onChange={setIs4k} />
      <Show when={error()}><p class="form-error" role="alert"><Icon name="alert" /> {error()}</p></Show>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Connecting…" : "Add & test"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
    </form>
  );
}

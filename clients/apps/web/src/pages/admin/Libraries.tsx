import { createResource, createSignal, For, Index, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, invalidateLibrary, type LibraryKind } from "../../api";
import { scans } from "../../lib/downloads";
import { toast } from "../../lib/toast";
import { Field } from "../../components/Form";
import { Panel } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

export default function Libraries(): JSX.Element {
  const [libs, { refetch }] = createResource(() => api.libraries());
  const [adding, setAdding] = createSignal(false);

  const scan = async (id: number, name: string) => {
    try {
      await api.scanLibrary(id);
      toast(`Scanning “${name}”…`);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't start the scan", "error");
    }
  };

  return (
    <>
      <Panel
        title="Libraries"
        icon="folder"
        aside={
          <Show when={!adding()}>
            <button type="button" class="btn btn--primary btn--sm" onClick={() => setAdding(true)}>
              <Icon name="plus" /> <span>Add library</span>
            </button>
          </Show>
        }
      >
        <Show when={adding()}>
          <AddLibrary
            onDone={(created) => {
              setAdding(false);
              if (created) {
                invalidateLibrary();
                void refetch();
              }
            }}
          />
        </Show>
        <Show when={!libs.error} fallback={<ErrorState error={libs.error} onRetry={refetch} />}>
          <Show when={libs() && !libs()!.length && !adding()}>
            <p class="panel__empty">No libraries yet. Add one to start scanning your media.</p>
          </Show>
          <ul class="list">
            <For each={libs()}>
              {(l) => {
                const s = () => scans()[l.id];
                const running = () => !!s() && s()!.scanned < s()!.total && Date.now() - s()!.at < 60_000;
                return (
                  <li class="list__row">
                    <Icon name={l.kind === "shows" ? "tv" : "film"} class="list__icon" />
                    <div class="list__main">
                      <strong>{l.name}</strong>
                      <span class="list__sub">
                        {l.kind === "shows" ? "Series" : "Films"} · {l.item_count.toLocaleString()} titles
                      </span>
                      <span class="list__paths">
                        <For each={l.paths}>{(p) => <code>{p}</code>}</For>
                      </span>
                      <Show when={running()}>
                        <div class="meter" aria-label={`Scanned ${s()!.scanned} of ${s()!.total}`}>
                          <i style={{ width: `${(s()!.scanned / Math.max(1, s()!.total)) * 100}%` }} />
                          <span>Scanning · {s()!.scanned} / {s()!.total}</span>
                        </div>
                      </Show>
                    </div>
                    <div class="list__actions">
                      <button type="button" class="btn btn--sm" aria-disabled={running() ? "true" : undefined} onClick={() => !running() && void scan(l.id, l.name)}>
                        <Icon name="retry" /> <span>{running() ? "Scanning…" : "Scan now"}</span>
                      </button>
                    </div>
                  </li>
                );
              }}
            </For>
          </ul>
        </Show>
      </Panel>
    </>
  );
}

function AddLibrary(props: { onDone: (created: boolean) => void }): JSX.Element {
  const [name, setName] = createSignal("");
  const [kind, setKind] = createSignal<LibraryKind>("movies");
  const [paths, setPaths] = createSignal<string[]>([""]);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    const ps = paths().map((p) => p.trim()).filter(Boolean);
    if (!name().trim()) return setError("Give the library a name.");
    if (!ps.length) return setError("Add at least one folder path.");
    setBusy(true);
    setError(undefined);
    try {
      const lib = await api.createLibrary({ name: name().trim(), kind: kind(), paths: ps });
      toast(`Added “${lib.name}” — scanning now`, "success");
      await api.scanLibrary(lib.id).catch(() => undefined);
      props.onDone(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Couldn't add the library");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="subform" onSubmit={submit} novalidate>
      <div class="form-grid">
        <Field label="Name" placeholder="Films" value={name()} onInput={(e) => setName(e.currentTarget.value)} data-autofocus />
        <div class="field">
          <span class="field__label">Type</span>
          <div class="segmented" role="group" aria-label="Library type">
            <button type="button" class="segmented__btn" aria-pressed={kind() === "movies"} onClick={() => setKind("movies")}>Films</button>
            <button type="button" class="segmented__btn" aria-pressed={kind() === "shows"} onClick={() => setKind("shows")}>Series</button>
          </div>
        </div>
      </div>
      <div class="field">
        <span class="field__label">Folders on the server</span>
        <Index each={paths()}>
          {(p, i) => (
            <div class="field__row">
              <input
                class="field__input field__input--mono"
                placeholder={kind() === "shows" ? "/media/series" : "/media/films"}
                value={p()}
                onInput={(e) => setPaths((ps) => ps.map((x, j) => (j === i ? e.currentTarget.value : x)))}
                aria-label={`Folder ${i + 1}`}
                spellcheck={false}
                autocapitalize="off"
              />
              <Show when={paths().length > 1}>
                <button type="button" class="iconbtn" aria-label="Remove folder" onClick={() => setPaths((ps) => ps.filter((_x, j) => j !== i))}>
                  <Icon name="x" />
                </button>
              </Show>
            </div>
          )}
        </Index>
        <button type="button" class="btn btn--quiet btn--sm" onClick={() => setPaths((ps) => ps.concat(""))}>
          <Icon name="plus" /> <span>Another folder</span>
        </button>
      </div>
      <Show when={error()}><p class="form-error" role="alert"><Icon name="alert" /> {error()}</p></Show>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Adding…" : "Add & scan"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone(false)}><span>Cancel</span></button>
      </div>
    </form>
  );
}

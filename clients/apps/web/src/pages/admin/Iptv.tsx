import { createMemo, createResource, createSignal, For, Match, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { formatBytes, Icon } from "@reel/ui";
import "../../styles/live.css";
import {
  iptv, type AdminChannel, type ChannelMetrics, type HubStatus, type SourceBody, type SourceDto, type SourcePatch, type SourceTest,
} from "../../api/iptv";
import { toast } from "../../lib/toast";
import { href } from "../../lib/mode";
import { Field, PasswordField } from "../../components/Form";
import { ConfirmButton, Panel, Select, Toggle, timeAgo } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

type Tab = "providers" | "channels" | "activity";

export default function Iptv(): JSX.Element {
  const [tab, setTab] = createSignal<Tab>("providers");
  const [sources, { refetch, mutate }] = createResource(() => iptv.sources());
  // Poll while a refresh runs so counts and timestamps update by themselves.
  onMount(() => {
    const t = setInterval(() => {
      if ((sources() ?? []).some((s) => s.refreshing)) void refetch();
    }, 2500);
    onCleanup(() => clearInterval(t));
  });
  return (
    <>
      <div class="segmented iptv-tabs" role="tablist" aria-label="Live TV settings" data-nav-group>
        <button type="button" role="tab" class="segmented__btn" aria-pressed={tab() === "providers"} aria-selected={tab() === "providers"} onClick={() => setTab("providers")}>Providers</button>
        <button type="button" role="tab" class="segmented__btn" aria-pressed={tab() === "channels"} aria-selected={tab() === "channels"} onClick={() => setTab("channels")}>Channels</button>
        <button type="button" role="tab" class="segmented__btn" aria-pressed={tab() === "activity"} aria-selected={tab() === "activity"} onClick={() => setTab("activity")}>Live activity</button>
      </div>
      <Switch>
        <Match when={tab() === "providers"}>
          <Providers sources={sources()} error={sources.error} refetch={refetch} mutate={(s) => mutate((l) => (l ?? []).map((x) => (x.id === s.id ? s : x)))} />
        </Match>
        <Match when={tab() === "channels"}>
          <Channels sources={sources() ?? []} />
        </Match>
        <Match when={tab() === "activity"}>
          <Hub sources={sources() ?? []} />
        </Match>
      </Switch>
    </>
  );
}

// ---------------------------------------------------------------------------------------
// Providers

function Providers(props: { sources?: SourceDto[]; error: unknown; refetch: () => void; mutate: (s: SourceDto) => void }): JSX.Element {
  const [adding, setAdding] = createSignal(false);
  return (
    <Panel
      title="TV providers"
      icon="live"
      aside={
        <Show when={!adding()}>
          <button type="button" class="btn btn--primary btn--sm" onClick={() => setAdding(true)}><Icon name="plus" /> <span>Add provider</span></button>
        </Show>
      }
    >
      <p class="panel__lede">
        Reel reads your channel list and guide from an M3U playlist or an Xtream Codes login, opens one connection per channel and shares it with
        everyone watching — so it never goes over your provider's connection limit. <a href={href("/live")}>Open Live TV</a>
      </p>
      <Show when={adding()}>
        <SourceForm
          onDone={(s) => {
            setAdding(false);
            if (s) {
              props.refetch();
              toast(`Added ${s.name} — loading channels`, "success");
            }
          }}
        />
      </Show>
      <Show when={!props.error} fallback={<ErrorState error={props.error} onRetry={props.refetch} />}>
        <ul class="list">
          <For
            each={props.sources}
            fallback={
              <Show when={props.sources && !adding()}>
                <li class="panel__empty iptv-empty">
                  <strong>No providers yet.</strong> Add your IPTV subscription to turn on Live TV and the guide.
                </li>
              </Show>
            }
          >
            {(s) => <SourceRow source={s} onChange={props.mutate} onDelete={props.refetch} />}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

function SourceRow(props: { source: SourceDto; onChange: (s: SourceDto) => void; onDelete: () => void }): JSX.Element {
  const s = () => props.source;
  const [editing, setEditing] = createSignal(false);
  const [testing, setTesting] = createSignal(false);
  const [result, setResult] = createSignal<SourceTest>();
  const [refreshing, setRefreshing] = createSignal(false);

  const test = async () => {
    if (testing()) return;
    setTesting(true);
    setResult(undefined);
    try {
      setResult(await iptv.testSource(s().id));
    } catch (e) {
      setResult({ ok: false, message: e instanceof Error ? e.message : "Test failed" });
    } finally {
      setTesting(false);
    }
  };
  const refresh = async () => {
    if (refreshing() || s().refreshing) return;
    setRefreshing(true);
    try {
      await iptv.refreshSource(s().id);
      props.onChange({ ...s(), refreshing: true });
      toast(`Refreshing ${s().name}…`, "info", 2500);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't start a refresh", "error");
    } finally {
      setRefreshing(false);
    }
  };
  const setEnabled = async (v: boolean) => {
    try {
      props.onChange(await iptv.updateSource(s().id, { enabled: v }));
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't save", "error");
    }
  };

  return (
    <li class="list__row list__row--stack iptv-src" classList={{ "is-disabled": !s().enabled }}>
      <div class="list__line">
        <span class={`brand brand--${s().kind === "xtream" ? "xtream" : "m3u"}`} aria-hidden="true">{s().kind === "xtream" ? "X" : "M"}</span>
        <div class="list__main">
          <strong>
            {s().name} <span class="pill pill--status">{s().kind === "xtream" ? "Xtream Codes" : "M3U"}</span>
          </strong>
          <span class="list__sub">
            <code>{s().url}</code>
            {s().username ? ` · ${s().username}` : ""}
          </span>
          <span class="iptv-src__status">
            <Show
              when={!s().refreshing}
              fallback={<span class="iptv-src__busy"><span class="spinner spinner--xs" /> Loading channels and guide…</span>}
            >
              <span><strong>{s().channel_count.toLocaleString()}</strong> channels</span>
              <span>{s().last_refresh_at ? `refreshed ${timeAgo(s().last_refresh_at!)}` : "never refreshed"}</span>
              <Show when={s().last_epg_at}><span>guide {timeAgo(s().last_epg_at!)}</span></Show>
            </Show>
            <ConnMeter open={s().open_connections} max={s().max_connections ?? null} />
          </span>
          <Show when={s().last_error}>
            <span class="form-warn"><Icon name="alert" /> {s().last_error}</span>
          </Show>
        </div>
        <div class="list__actions">
          <button type="button" class="btn btn--sm" aria-disabled={testing() ? "true" : undefined} onClick={() => void test()}>
            <Icon name="activity" /> <span>{testing() ? "Testing…" : "Test"}</span>
          </button>
          <button type="button" class="btn btn--sm" aria-disabled={s().refreshing || refreshing() ? "true" : undefined} onClick={() => void refresh()}>
            <Icon name="retry" /> <span>{s().refreshing ? "Refreshing…" : "Refresh now"}</span>
          </button>
          <button type="button" class="btn btn--sm btn--quiet" aria-expanded={editing()} onClick={() => setEditing((v) => !v)}>
            <Icon name="gear" /> <span>Edit</span>
          </button>
        </div>
      </div>
      <Show when={result()}>{(r) => <TestResult result={r()} />}</Show>
      <Show when={editing()}>
        <SourceForm
          source={s()}
          onDone={(n) => {
            setEditing(false);
            if (n) {
              props.onChange(n);
              toast("Saved", "success", 1800);
            }
          }}
          extra={
            <>
              <Toggle label="Enabled" hint="Turn off to hide this provider's channels without deleting them" checked={s().enabled} onChange={(v) => void setEnabled(v)} />
              <ConfirmButton
                label="Remove provider"
                confirm="Remove it and all its channels?"
                onConfirm={async () => {
                  try {
                    await iptv.deleteSource(s().id);
                    props.onDelete();
                    toast(`Removed ${s().name}`);
                  } catch (e) {
                    toast(e instanceof Error ? e.message : "Couldn't remove", "error");
                  }
                }}
              />
            </>
          }
        />
      </Show>
    </li>
  );
}

function ConnMeter(props: { open: number; max: number | null }): JSX.Element {
  const pct = () => (props.max ? Math.min(1, props.open / props.max) : 0);
  return (
    <span class="connmeter" classList={{ "is-full": !!props.max && props.open >= props.max }} title="Upstream connections open now (watching + kept warm)">
      <Show when={props.max} fallback={<span>{props.open} connection{props.open === 1 ? "" : "s"} open · no limit set</span>}>
        <span class="connmeter__bar" aria-hidden="true"><i style={{ width: `${pct() * 100}%` }} /></span>
        <span>{props.open} of {props.max} connections</span>
      </Show>
    </span>
  );
}

function TestResult(props: { result: SourceTest; onUseLimit?: (n: number) => void }): JSX.Element {
  const r = () => props.result;
  const a = () => r().account ?? null;
  return (
    <div class="testres" classList={{ "is-ok": r().ok, "is-bad": !r().ok }} role="status">
      <Icon name={r().ok ? "check" : "alert"} />
      <div class="testres__body">
        <strong>{r().message}</strong>
        <div class="testres__facts">
          <Show when={r().channels != null}><span>{r().channels!.toLocaleString()} channels found</span></Show>
          <Show when={a()}>
            {(acc) => (
              <>
                <span>Account {(acc().status ?? (acc().active ? "active" : "inactive")).toLowerCase()}{acc().is_trial ? " (trial)" : ""}</span>
                <Show when={acc().exp_date}><span>expires {new Date(acc().exp_date! * 1000).toLocaleDateString()}</span></Show>
                <Show when={acc().max_connections != null}>
                  <span>
                    {acc().max_connections} connection{acc().max_connections === 1 ? "" : "s"} allowed
                    {acc().active_connections != null ? `, ${acc().active_connections} in use` : ""}
                  </span>
                </Show>
              </>
            )}
          </Show>
          <Show when={r().epg_url}><span>guide: <code>{r().epg_url}</code></span></Show>
        </div>
        <Show when={props.onUseLimit && a()?.max_connections}>
          <button type="button" class="btn btn--sm" onClick={() => props.onUseLimit!(a()!.max_connections!)}>
            <Icon name="check" /> <span>Use limit of {a()!.max_connections}</span>
          </button>
        </Show>
      </div>
    </div>
  );
}

const REFRESH_OPTS = [
  { value: "", label: "Every 12 hours (default)" },
  { value: "180", label: "Every 3 hours" },
  { value: "360", label: "Every 6 hours" },
  { value: "1440", label: "Once a day" },
  { value: "0", label: "Only when I press Refresh" },
];

function SourceForm(props: { source?: SourceDto; onDone: (s?: SourceDto) => void; extra?: JSX.Element }): JSX.Element {
  const src = props.source;
  const [kind, setKind] = createSignal<"xtream" | "m3u">(src?.kind === "m3u" ? "m3u" : src ? "xtream" : "xtream");
  const [name, setName] = createSignal(src?.name ?? "");
  const [url, setUrl] = createSignal(src?.url ?? "");
  const [username, setUsername] = createSignal(src?.username ?? "");
  const [password, setPassword] = createSignal("");
  const [epg, setEpg] = createSignal(src?.epg_url ?? "");
  const [maxConn, setMaxConn] = createSignal(src?.max_connections != null ? String(src.max_connections) : "");
  const [interval, setIntervalV] = createSignal(src?.refresh_interval != null ? String(src.refresh_interval) : "");
  const [ua, setUa] = createSignal(src?.user_agent ?? "");
  const [more, setMore] = createSignal(!!src?.user_agent);
  const [busy, setBusy] = createSignal(false);
  const [testing, setTesting] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [result, setResult] = createSignal<SourceTest>();

  const body = (): SourceBody => ({
    kind: kind(),
    name: name().trim() || (kind() === "xtream" ? hostOf(url()) || "IPTV" : "M3U playlist"),
    url: url().trim(),
    username: kind() === "xtream" ? username().trim() || null : null,
    password: kind() === "xtream" && password() ? password() : null,
    epg_url: epg().trim() || null,
    max_connections: maxConn().trim() === "" ? null : Math.max(0, Math.floor(Number(maxConn())) || 0),
    refresh_interval: interval() === "" ? null : Number(interval()),
    user_agent: ua().trim() || null,
  });

  const validate = (): string | undefined => {
    if (!url().trim()) return kind() === "xtream" ? "Enter your provider's server address." : "Enter the playlist URL.";
    if (!/^https?:\/\//i.test(url().trim())) return "The address should start with http:// or https://.";
    if (kind() === "xtream" && !username().trim()) return "Xtream logins need a username.";
    if (kind() === "xtream" && !password() && !src?.password_set) return "Xtream logins need a password.";
    return undefined;
  };

  const test = async () => {
    if (testing()) return;
    const v = validate();
    if (v) return setError(v);
    setError(undefined);
    setTesting(true);
    setResult(undefined);
    try {
      // Editing without retyping the password: test the saved configuration instead.
      const r = src && !password() && src.password_set && kind() === "xtream" ? await iptv.testSource(src.id) : await iptv.testConfig(body());
      setResult(r);
      if (r.ok && maxConn().trim() === "" && r.account?.max_connections) setMaxConn(String(r.account.max_connections));
      if (r.ok && !epg().trim() && r.epg_url && !src) setEpg("");
    } catch (e) {
      setResult({ ok: false, message: e instanceof Error ? e.message : "Test failed" });
    } finally {
      setTesting(false);
    }
  };

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    const v = validate();
    if (v) return setError(v);
    setBusy(true);
    setError(undefined);
    try {
      if (src) {
        const b = body();
        const patch: SourcePatch = {
          name: b.name, url: b.url, username: b.username ?? "", epg_url: b.epg_url ?? "", max_connections: b.max_connections ?? 0,
          refresh_interval: b.refresh_interval, user_agent: b.user_agent ?? "",
        };
        if (password()) patch.password = password();
        props.onDone(await iptv.updateSource(src.id, patch));
      } else {
        props.onDone(await iptv.createSource({ ...body(), enabled: true }));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Couldn't save the provider");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="subform iptv-form" onSubmit={submit} novalidate>
      <div class="segmented" role="group" aria-label="Provider type">
        <button type="button" class="segmented__btn" aria-pressed={kind() === "xtream"} onClick={() => setKind("xtream")}>Xtream Codes login</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "m3u"} onClick={() => setKind("m3u")}>M3U playlist URL</button>
      </div>
      <div class="form-grid">
        <Field label="Name" placeholder={kind() === "xtream" ? "My provider" : "Playlist"} value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <Show
          when={kind() === "xtream"}
          fallback={
            <Field
              label="Playlist URL"
              placeholder="https://provider.example/get.php?username=…&type=m3u_plus"
              hint="The .m3u / .m3u8 link from your provider"
              value={url()}
              onInput={(e) => setUrl(e.currentTarget.value)}
              autocapitalize="off"
              spellcheck={false}
              inputmode="url"
              data-autofocus
            />
          }
        >
          <Field
            label="Server address"
            placeholder="http://line.provider.example:8080"
            hint="Just the server — no /player_api.php"
            value={url()}
            onInput={(e) => setUrl(e.currentTarget.value)}
            autocapitalize="off"
            spellcheck={false}
            inputmode="url"
            data-autofocus
          />
          <Field label="Username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocapitalize="off" spellcheck={false} autocomplete="off" />
          <PasswordField
            label="Password"
            placeholder={src?.password_set ? "Saved — type to change" : ""}
            value={password()}
            onInput={(e) => setPassword(e.currentTarget.value)}
            autocomplete="new-password"
          />
        </Show>
        <Field
          label="Guide (XMLTV) URL"
          placeholder={src?.detected_epg_url ?? "Optional"}
          hint={src?.detected_epg_url ? "Leave empty to use the one your provider advertises" : "Optional — most providers advertise their own"}
          value={epg()}
          onInput={(e) => setEpg(e.currentTarget.value)}
          autocapitalize="off"
          spellcheck={false}
          inputmode="url"
        />
        <div class="field field--num">
          <label class="field__label" for="iptv-maxconn">Connection limit</label>
          <input
            id="iptv-maxconn"
            class="field__input"
            type="number"
            min="0"
            inputmode="numeric"
            placeholder="Unknown"
            value={maxConn()}
            onInput={(e) => setMaxConn(e.currentTarget.value)}
          />
          <span class="field__hint">How many channels your plan can stream at once. Test fills this in for Xtream.</span>
        </div>
        <Select label="Refresh channels & guide" value={interval()} onChange={(e) => setIntervalV(e.currentTarget.value)} options={REFRESH_OPTS} />
      </div>
      <Show when={more()} fallback={<button type="button" class="btn btn--sm btn--quiet iptv-more" onClick={() => setMore(true)}><Icon name="plus" /> <span>More options</span></button>}>
        <div class="form-grid">
          <Field label="User agent" placeholder="Default" hint="Only if your provider requires a specific player" value={ua()} onInput={(e) => setUa(e.currentTarget.value)} autocapitalize="off" spellcheck={false} />
        </div>
      </Show>
      <Show when={result()}>{(r) => <TestResult result={r()} onUseLimit={maxConn() ? undefined : (n) => setMaxConn(String(n))} />}</Show>
      <Show when={error()}><p class="form-error" role="alert"><Icon name="alert" /> {error()}</p></Show>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}>
          <Icon name="check" /> <span>{busy() ? "Saving…" : src ? "Save changes" : "Add provider"}</span>
        </button>
        <button type="button" class="btn" aria-disabled={testing() ? "true" : undefined} onClick={() => void test()}>
          <Icon name="activity" /> <span>{testing() ? "Testing…" : "Test connection"}</span>
        </button>
        <button type="button" class="btn btn--quiet" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
      <Show when={props.extra}><div class="iptv-form__extra">{props.extra}</div></Show>
    </form>
  );
}

function hostOf(u: string): string {
  try {
    return new URL(u.trim()).hostname.replace(/^(www|line)\./, "");
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------------------
// Channels

const PAGE = 80;

function Channels(props: { sources: SourceDto[] }): JSX.Element {
  const [q, setQ] = createSignal("");
  const [source, setSource] = createSignal("");
  const [group, setGroup] = createSignal("");
  const [hidden, setHidden] = createSignal(true);
  const [shown, setShown] = createSignal(PAGE);
  const [list, { refetch, mutate }] = createResource(
    () => ({ source: source() ? Number(source()) : undefined, hidden: hidden() }),
    (o) => iptv.adminChannels(o),
  );
  const groups = createMemo(() => {
    const s = new Set<string>();
    (list() ?? []).forEach((c) => c.group && s.add(c.group));
    return Array.from(s).sort();
  });
  const rows = createMemo(() => {
    const t = q().trim().toLowerCase();
    return (list() ?? []).filter(
      (c) =>
        (!group() || c.group === group()) &&
        (!t || c.name.toLowerCase().indexOf(t) >= 0 || String(c.number ?? "") === t || (c.epg_channel_id ?? "").toLowerCase().indexOf(t) >= 0),
    );
  });
  const stats = createMemo(() => {
    const l = list() ?? [];
    return { total: l.length, off: l.filter((c) => !c.enabled).length, noEpg: l.filter((c) => !c.epg_channel_id).length };
  });
  const update = (c: AdminChannel) => mutate((l) => (l ?? []).map((x) => (x.id === c.id ? c : x)));

  return (
    <Panel title="Channels" icon="tv" aside={<span class="panel__live">{stats().total.toLocaleString()} total · {stats().off} off · {stats().noEpg} without guide</span>}>
      <p class="panel__lede">Turn channels off to hide them, give them your own numbers, or point one at the right guide entry when the automatic match is wrong.</p>
      <div class="chfilters">
        <Field label="Search" type="search" placeholder="Name, number or guide id" value={q()} onInput={(e) => { setQ(e.currentTarget.value); setShown(PAGE); }} />
        <Show when={props.sources.length > 1}>
          <Select label="Provider" value={source()} onChange={(e) => setSource(e.currentTarget.value)} options={[{ value: "", label: "All providers" }].concat(props.sources.map((s) => ({ value: String(s.id), label: s.name })))} />
        </Show>
        <Select label="Group" value={group()} onChange={(e) => { setGroup(e.currentTarget.value); setShown(PAGE); }} options={[{ value: "", label: "All groups" }].concat(groups().map((g) => ({ value: g, label: g })))} />
        <Toggle label="Show hidden" hint="Turned off or gone from the provider" checked={hidden()} onChange={setHidden} />
      </div>
      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={refetch} />}>
        <Show when={!list.loading || list()} fallback={<p class="panel__empty">Loading channels…</p>}>
          <ul class="list chman">
            <For each={rows().slice(0, shown())} fallback={<li class="panel__empty">No channels match.</li>}>
              {(c) => <ChannelRow channel={c} onChange={update} />}
            </For>
          </ul>
          <Show when={rows().length > shown()}>
            <div class="actions">
              <button type="button" class="btn btn--sm" onClick={() => setShown((n) => n + PAGE * 2)}>
                <Icon name="down" /> <span>Show more ({(rows().length - shown()).toLocaleString()} left)</span>
              </button>
            </div>
          </Show>
        </Show>
      </Show>
    </Panel>
  );
}

function ChannelRow(props: { channel: AdminChannel; onChange: (c: AdminChannel) => void }): JSX.Element {
  const c = () => props.channel;
  const [open, setOpen] = createSignal(false);
  const [name, setName] = createSignal(c().name);
  const [num, setNum] = createSignal(c().number != null ? String(c().number) : "");
  const [group, setGroup] = createSignal(c().group ?? "");
  const [epg, setEpg] = createSignal(c().epg_manual ? c().epg_channel_id ?? "" : "");
  const [busy, setBusy] = createSignal(false);

  const save = async (patch: Parameters<typeof iptv.patchChannel>[1], local: Partial<AdminChannel>) => {
    try {
      await iptv.patchChannel(c().id, patch);
      props.onChange({ ...c(), ...local });
      return true;
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't save", "error");
      return false;
    }
  };
  const saveAll = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    setBusy(true);
    const n = num().trim() === "" ? null : Math.max(0, Math.floor(Number(num())) || 0);
    const ep = epg().trim() || null;
    const ok = await save(
      { name: name().trim() || c().name, number: n, group: group().trim() || null, epg_channel_id: ep },
      { name: name().trim() || c().name, number: n, group: group().trim() || null, epg_channel_id: ep ?? c().tvg_id ?? null, epg_manual: ep != null },
    );
    setBusy(false);
    if (ok) {
      setOpen(false);
      toast("Channel saved", "success", 1600);
    }
  };

  return (
    <li class="list__row list__row--stack chman__row" classList={{ "is-disabled": !c().enabled || c().missing }}>
      <div class="list__line">
        <span class="chman__num">{c().number ?? "–"}</span>
        <div class="list__main">
          <strong>
            {c().name}
            <Show when={c().missing}> <span class="pill pill--disabled">Gone from provider</span></Show>
          </strong>
          <span class="list__sub">
            {c().group ?? "No group"}
            {" · "}
            <Show when={c().epg_channel_id} fallback={<span class="chman__noepg">no guide match</span>}>
              guide <code>{c().epg_channel_id}</code>
              <Show when={c().epg_manual}> <span class="pill pill--status">manual</span></Show>
            </Show>
            <Show when={c().aliases.length}> · {c().aliases.length} backup{c().aliases.length === 1 ? "" : "s"}</Show>
          </span>
        </div>
        <div class="list__actions">
          <Toggle label={c().enabled ? "On" : "Off"} checked={c().enabled} onChange={(v) => void save({ enabled: v }, { enabled: v })} />
          <button type="button" class="btn btn--sm btn--quiet" aria-expanded={open()} onClick={() => setOpen((v) => !v)}>
            <Icon name="gear" /> <span>Edit</span>
          </button>
        </div>
      </div>
      <Show when={open()}>
        <form class="subform" onSubmit={saveAll} novalidate>
          <div class="form-grid">
            <Field label="Name" value={name()} onInput={(e) => setName(e.currentTarget.value)} data-autofocus />
            <Field label="Number" type="number" min="0" inputmode="numeric" placeholder="None" value={num()} onInput={(e) => setNum(e.currentTarget.value)} />
            <Field label="Group" value={group()} onInput={(e) => setGroup(e.currentTarget.value)} />
            <Field
              label="Guide channel id"
              placeholder={c().tvg_id ?? "Automatic"}
              hint="The XMLTV id to read the guide from. Empty = automatic matching."
              value={epg()}
              onInput={(e) => setEpg(e.currentTarget.value)}
              autocapitalize="off"
              spellcheck={false}
            />
          </div>
          <div class="actions">
            <button type="submit" class="btn btn--primary btn--sm" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Saving…" : "Save"}</span></button>
            <button type="button" class="btn btn--sm" onClick={() => setOpen(false)}><span>Cancel</span></button>
          </div>
        </form>
      </Show>
    </li>
  );
}

// ---------------------------------------------------------------------------------------
// Hub

function Hub(props: { sources: SourceDto[] }): JSX.Element {
  const [hub, { refetch, mutate }] = createResource(() => iptv.hub());
  onMount(() => {
    const t = setInterval(() => void refetch(), 3000);
    onCleanup(() => clearInterval(t));
  });
  const name = (id: number) => props.sources.find((s) => s.id === id)?.name ?? `Provider ${id}`;
  const stop = async (id: string) => {
    try {
      await iptv.stop(id);
      mutate((h) => (h ? { ...h, sessions: h.sessions.filter((s) => s.id !== id) } : h));
      toast("Stopped the stream");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't stop it", "error");
    }
  };
  return (
    <Panel title="Live activity" icon="activity" aside={<span class="panel__live"><i /> Live · every 3s</span>}>
      <Show when={!hub.error} fallback={<ErrorState error={hub.error} onRetry={refetch} />}>
        <Show when={hub()} fallback={<p class="panel__empty">Loading…</p>}>
          {(h) => <HubBody hub={h()} name={name} onStop={stop} />}
        </Show>
      </Show>
    </Panel>
  );
}

function HubBody(props: { hub: HubStatus; name: (id: number) => string; onStop: (id: string) => void }): JSX.Element {
  const h = () => props.hub;
  return (
    <>
      <div class="hub__sources">
        <For each={h().sources} fallback={<p class="panel__empty">No providers.</p>}>
          {(s) => (
            <div class="hub__src">
              <span class="hub__src-name">{props.name(s.source_id)}</span>
              <ConnMeter open={s.open_connections} max={s.max_connections ?? null} />
            </div>
          )}
        </For>
        <p class="hub__enc">
          <Icon name="chip" /> Transcoding (only for channels the browser can't play as-is): <strong>{h().transcode.encoder.toUpperCase()}</strong>
          <Show when={h().transcode.fell_back_to_software}> <span class="pill pill--disabled">fell back to software</span></Show>
        </p>
      </div>

      <h3 class="hub__h">Open channels</h3>
      <Show when={h().channels.length} fallback={<p class="panel__empty">No channels open. They appear here when someone tunes in, and stay warm for a moment after.</p>}>
        <div class="hub__tablewrap">
          <table class="hub__table">
            <thead>
              <tr>
                <th>Channel</th><th>State</th><th class="num">Viewers</th><th class="num">Bitrate</th><th class="num">First byte</th><th class="num">Keyframe</th><th>Codecs</th><th class="num">Health</th>
              </tr>
            </thead>
            <tbody>
              <For each={h().channels}>{(c) => <MetricRow m={c} />}</For>
            </tbody>
          </table>
        </div>
      </Show>

      <h3 class="hub__h">Who's watching</h3>
      <ul class="list">
        <For each={h().sessions} fallback={<li class="panel__empty">Nobody is watching live TV.</li>}>
          {(s) => (
            <li class="list__row">
              <span class="avatar" aria-hidden="true">{(s.username ?? "?").charAt(0).toUpperCase()}</span>
              <div class="list__main">
                <strong>{s.channel_name}</strong>
                <span class="list__sub">
                  {s.username ?? `user ${s.user_id}`} · started {timeAgo(s.started_at)}
                  {s.idle_s > 20 ? ` · idle ${s.idle_s}s` : ""}
                  {s.hls ? ` · video ${s.hls.video}, audio ${s.hls.audio}` : ""}
                </span>
              </div>
              <div class="list__actions">
                <span class="pill pill--status">{s.mode === "passthrough" ? "Passthrough" : "HLS"}</span>
                <ConfirmButton label="Stop" icon="x" confirm="Stop this stream?" onConfirm={() => props.onStop(s.id)} />
              </div>
            </li>
          )}
        </For>
      </ul>
    </>
  );
}

function MetricRow(props: { m: ChannelMetrics }): JSX.Element {
  const m = () => props.m;
  const ms = (v?: number | null) => (v == null ? "—" : v >= 1000 ? `${(v / 1000).toFixed(1)} s` : `${v} ms`);
  const health = () => {
    const issues = m().reconnects + m().failovers + m().cc_errors + m().pts_discontinuities + m().sync_losses;
    return issues;
  };
  return (
    <tr classList={{ "is-warm": m().warm && !m().viewers }}>
      <td>
        <strong>{m().name}</strong>
        <Show when={m().active_candidate > 0}><span class="pill pill--status">backup #{m().active_candidate}</span></Show>
        <Show when={m().last_error}><span class="hub__err" title={m().last_error!}><Icon name="alert" /></span></Show>
      </td>
      <td><span class={`hubstate hubstate--${m().state}`}>{m().viewers ? m().state : "warm"}</span></td>
      <td class="num">{m().viewers}</td>
      <td class="num">{m().bitrate_kbps ? `${(m().bitrate_kbps / 1000).toFixed(1)} Mbps` : "—"}</td>
      <td class="num">{ms(m().ttfb_ms)}</td>
      <td class="num">{ms(m().time_to_keyframe_ms)}</td>
      <td>{[m().video_codec, m().audio_codec].filter(Boolean).join(" / ") || "—"}</td>
      <td class="num" title={`${m().reconnects} reconnects · ${m().failovers} failovers · ${m().cc_errors} CC errors · ${m().pts_discontinuities} PTS jumps · ${formatBytes(m().bytes_total)} received`}>
        <span classList={{ "hub__ok": !health(), "hub__warn": !!health() }}>{health() ? `${health()} issue${health() === 1 ? "" : "s"}` : "Clean"}</span>
      </td>
    </tr>
  );
}

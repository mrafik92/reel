import { createEffect, createMemo, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { formatBytes, Icon } from "@reel/ui";
import "../../styles/live.css";
import {
  iptv, type AdminChannel, type ChannelMetrics, type HubStatus, type SourceBody, type SourceDto, type SourcePatch, type SourceTest,
} from "../../api/iptv";
import { ApiError } from "../../api/http";
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
          <Providers
            sources={sources()}
            error={sources.error}
            refetch={refetch}
            update={(s) => mutate((l) => (l ?? []).map((x) => (x.id === s.id ? s : x)))}
            add={(s) => mutate((l) => (l ?? []).filter((x) => x.id !== s.id).concat(s))}
            remove={(id) => mutate((l) => (l ?? []).filter((x) => x.id !== id))}
          />
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

function Providers(props: {
  sources?: SourceDto[];
  error: unknown;
  refetch: () => void;
  update: (s: SourceDto) => void;
  add: (s: SourceDto) => void;
  remove: (id: number) => void;
}): JSX.Element {
  const [adding, setAdding] = createSignal(false);
  // The provider just added: its row follows the first channel/guide load.
  const [fresh, setFresh] = createSignal<number>();
  // Rows are keyed by id: list refreshes replace the objects but must not remount a row
  // (it would lose its edit form and the load it's following).
  const ids = createMemo(() => (props.sources ?? []).map((s) => s.id), undefined, {
    equals: (a, b) => a.length === b.length && a.every((v, i) => v === b[i]),
  });
  const byId = (id: number) => (props.sources ?? []).find((s) => s.id === id);
  // First provider: open the form straight away.
  let opened = false;
  createEffect(() => {
    if (!opened && props.sources && props.sources.length === 0) {
      opened = true;
      setAdding(true);
    }
  });
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
        Reel reads your channel list and guide from an Xtream Codes login or an M3U playlist, opens one connection per channel and shares it with
        everyone watching — so it never goes over your provider's connection limit. <a href={href("/live")}>Open Live TV</a>
      </p>
      <Show when={adding()}>
        <SourceForm
          onDone={(s) => {
            setAdding(false);
            if (s) {
              setFresh(s.id);
              props.add(s);
            }
          }}
        />
      </Show>
      <Show when={!props.error} fallback={<ErrorState error={props.error} onRetry={props.refetch} />}>
        <ul class="list">
          <For
            each={ids()}
            fallback={
              <Show when={props.sources && !adding()}>
                <li class="panel__empty iptv-empty">
                  <strong>No providers yet.</strong> Add your IPTV subscription to turn on Live TV and the guide.
                </li>
              </Show>
            }
          >
            {(id) => (
              <Show when={byId(id)}>
                {(s) => <SourceRow source={s()} follow={fresh() === id} onChange={props.update} onDelete={() => props.remove(id)} />}
              </Show>
            )}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

interface Job {
  /** `last_refresh_at` before we started — a new value means the load finished. */
  prev: number | null;
  started: number;
  /** Saw `refreshing: true` (or the server said one is running). */
  seen: boolean;
  kicked: boolean;
}
interface Outcome {
  ok: boolean;
  channels?: number;
  withGuide?: number;
  message?: string;
}

const nowS = () => Math.floor(Date.now() / 1000);

function SourceRow(props: { source: SourceDto; follow: boolean; onChange: (s: SourceDto) => void; onDelete: () => void }): JSX.Element {
  const s = () => props.source;
  const [editing, setEditing] = createSignal(false);
  const [testing, setTesting] = createSignal(false);
  const [result, setResult] = createSignal<SourceTest>();
  const [job, setJob] = createSignal<Job>();
  const [elapsed, setElapsed] = createSignal(0);
  const [outcome, setOutcome] = createSignal<Outcome>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let ticker: ReturnType<typeof setInterval> | undefined;
  onCleanup(() => {
    if (timer) clearTimeout(timer);
    if (ticker) clearInterval(ticker);
  });

  /** Follow a channel + guide load until it finishes, then summarise it. */
  const follow = (alreadyRunning = false) => {
    if (timer) clearTimeout(timer);
    setOutcome(undefined);
    const j: Job = { prev: s().last_refresh_at ?? null, started: nowS(), seen: alreadyRunning, kicked: alreadyRunning };
    setJob(j);
    setElapsed(0);
    if (ticker) clearInterval(ticker);
    ticker = setInterval(() => setElapsed(nowS() - j.started), 1000);
    timer = setTimeout(() => void poll(j), 1200);
  };
  const finish = (o: Outcome) => {
    setJob(undefined);
    if (ticker) clearInterval(ticker);
    setOutcome(o);
  };
  const poll = async (j: Job) => {
    if (job() !== j) return;
    let cur: SourceDto;
    try {
      cur = await iptv.source(s().id);
    } catch {
      timer = setTimeout(() => void poll(j), 3000);
      return;
    }
    if (job() !== j) return;
    props.onChange(cur);
    const since = nowS() - j.started;
    if (cur.refreshing) {
      j.seen = true;
    } else if (cur.last_refresh_at != null && cur.last_refresh_at !== j.prev) {
      // Done: count how many channels found their guide.
      let withGuide: number | undefined;
      try {
        const ch = await iptv.adminChannels({ source: cur.id, hidden: false });
        if (ch.length) withGuide = ch.filter((c) => !!c.epg_channel_id).length;
      } catch {
        /* the summary still has the channel count */
      }
      finish({ ok: true, channels: cur.channel_count, withGuide, message: cur.last_error ?? undefined });
      return;
    } else if (j.seen) {
      finish({ ok: false, message: cur.last_error || "Loading stopped before it finished." });
      return;
    } else if (since >= 6 && !j.kicked) {
      // The background load never showed up (or ended before we looked): start one.
      j.kicked = true;
      try {
        await iptv.refreshSource(cur.id);
        j.seen = true;
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) j.seen = true;
      }
    } else if (since >= 20 && !j.seen) {
      finish({ ok: false, message: cur.last_error || "Reel couldn't start loading this provider." });
      return;
    }
    if (since > 15 * 60) {
      finish({ ok: false, message: "Still loading after 15 minutes. It carries on in the background — check back later." });
      return;
    }
    timer = setTimeout(() => void poll(j), 2000);
  };

  onMount(() => {
    if (props.follow) follow();
    else if (s().refreshing) follow(true);
  });

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
    if (job()) return;
    try {
      await iptv.refreshSource(s().id);
      follow(true);
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) follow(true);
      else toast(e instanceof Error ? e.message : "Couldn't start a refresh", "error");
    }
  };
  const setEnabled = async (v: boolean) => {
    try {
      props.onChange(await iptv.updateSource(s().id, { enabled: v }));
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't save", "error");
    }
  };
  const busy = () => !!job() || s().refreshing;
  const clock = () => `${Math.floor(elapsed() / 60)}:${String(elapsed() % 60).padStart(2, "0")}`;

  return (
    <li class="list__row list__row--stack iptv-src" classList={{ "is-disabled": !s().enabled }}>
      <div class="list__line">
        <span class={`brand brand--${s().kind === "xtream" ? "xtream" : "m3u"}`} aria-hidden="true">{s().kind === "xtream" ? "X" : "M"}</span>
        <div class="list__main">
          <strong>
            {s().name} <span class="pill pill--status">{s().kind === "xtream" ? "Xtream Codes" : "M3U"}</span>
          </strong>
          <span class="list__sub">
            <code>{redact(s().url)}</code>
            {s().username ? ` · ${s().username}` : ""}
          </span>
          <span class="iptv-src__status">
            <Show when={!busy()} fallback={<span class="iptv-src__busy"><span class="spinner spinner--xs" /> Loading channels and guide…</span>}>
              <span><strong>{s().channel_count.toLocaleString()}</strong> channels</span>
              <span>{s().last_refresh_at ? `refreshed ${timeAgo(s().last_refresh_at!)}` : "never refreshed"}</span>
              <Show when={s().last_epg_at}><span>guide {timeAgo(s().last_epg_at!)}</span></Show>
            </Show>
            <ConnMeter open={s().open_connections} max={s().max_connections ?? null} />
          </span>
          <Show when={s().last_error && !busy() && !outcome()}>
            <span class="form-warn"><Icon name="alert" /> {redact(s().last_error)}</span>
          </Show>
        </div>
        <div class="list__actions">
          <button type="button" class="btn btn--sm" aria-disabled={testing() ? "true" : undefined} onClick={() => void test()}>
            <Icon name="activity" /> <span>{testing() ? "Testing…" : "Test"}</span>
          </button>
          <button type="button" class="btn btn--sm" aria-disabled={busy() ? "true" : undefined} onClick={() => void refresh()}>
            <Icon name="retry" /> <span>{busy() ? "Refreshing…" : "Refresh now"}</span>
          </button>
          <button type="button" class="btn btn--sm btn--quiet" aria-expanded={editing()} onClick={() => setEditing((v) => !v)}>
            <Icon name="gear" /> <span>Edit</span>
          </button>
        </div>
      </div>

      <Show when={job()}>
        <div class="loadjob" role="status" aria-live="polite">
          <div class="loadjob__head">
            <strong>Loading channels and guide from {s().name}</strong>
            <span class="loadjob__clock">{clock()}</span>
          </div>
          <div class="loadjob__bar" aria-hidden="true"><i /></div>
          <p class="loadjob__hint">
            {elapsed() < 25
              ? "Reading the channel list…"
              : elapsed() < 90
                ? "Reading the TV guide — big guides take a minute or two."
                : "Still working. You can leave this page; it carries on in the background."}
          </p>
        </div>
      </Show>

      <Show when={outcome()}>
        {(o) => (
          <div class="testres" classList={{ "is-ok": o().ok, "is-bad": !o().ok }} role="status">
            <Icon name={o().ok ? "check" : "alert"} />
            <div class="testres__body">
              <Show
                when={o().ok}
                fallback={
                  <>
                    <strong>Couldn't load this provider</strong>
                    <div class="testres__facts"><span>{redact(o().message)}</span></div>
                  </>
                }
              >
                <strong>
                  Ready — {(o().channels ?? 0).toLocaleString()} channel{o().channels === 1 ? "" : "s"}
                  <Show when={o().withGuide != null}>{o().withGuide ? `, guide found for ${o().withGuide!.toLocaleString()}` : " — no guide matched yet"}</Show>
                </strong>
                <Show when={o().message}>
                  <div class="testres__facts"><span class="form-warn"><Icon name="alert" /> {redact(o().message)}</span></div>
                </Show>
              </Show>
              <div class="actions testres__actions">
                <Show
                  when={o().ok}
                  fallback={
                    <>
                      <button type="button" class="btn btn--sm btn--primary" onClick={() => void refresh()}><Icon name="retry" /> <span>Try again</span></button>
                      <button type="button" class="btn btn--sm" onClick={() => setEditing(true)}><Icon name="gear" /> <span>Check settings</span></button>
                    </>
                  }
                >
                  <a class="btn btn--sm btn--primary" href={href("/live")}><Icon name="live" /> <span>Open Live TV</span></a>
                  <a class="btn btn--sm" href={href("/live/guide")}><Icon name="grid" /> <span>Guide</span></a>
                </Show>
                <button type="button" class="btn btn--sm btn--quiet" onClick={() => setOutcome(undefined)}><span>Dismiss</span></button>
              </div>
            </div>
          </div>
        )}
      </Show>

      <Show when={result()}>{(r) => <TestResult result={r()} />}</Show>
      <Show when={editing()}>
        <SourceForm
          source={s()}
          onDone={(n, reload) => {
            setEditing(false);
            if (!n) return;
            props.onChange(n);
            if (reload) void refresh();
            else toast("Saved", "success", 1800);
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

function fmtDate(unix: number): string {
  try {
    return new Date(unix * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
  } catch {
    return new Date(unix * 1000).toDateString();
  }
}

/** Test outcome: clear verdict first, then the Xtream account facts. */
function TestResult(props: { result: SourceTest; filledLimit?: number | null }): JSX.Element {
  const r = () => props.result;
  const a = () => r().account ?? null;
  const days = () => {
    const e = a()?.exp_date;
    return e ? Math.floor((e - nowS()) / 86400) : null;
  };
  const expired = () => days() != null && days()! < 0;
  const ok = () => r().ok && !expired() && (a() ? a()!.active : true);
  return (
    <div class="testres" classList={{ "is-ok": ok(), "is-bad": !ok() }} role="status" aria-live="polite">
      <Icon name={ok() ? "check" : "alert"} />
      <div class="testres__body">
        <strong>{redact(r().message)}</strong>
        <Show when={a()}>
          {(acc) => (
            <dl class="acct">
              <div>
                <dt>Account</dt>
                <dd classList={{ "is-bad": !acc().active }}>
                  {acc().status ? acc().status!.charAt(0).toUpperCase() + acc().status!.slice(1).toLowerCase() : acc().active ? "Active" : "Inactive"}
                  {acc().is_trial ? " · trial" : ""}
                </dd>
              </div>
              <Show when={acc().exp_date}>
                <div>
                  <dt>{expired() ? "Expired" : "Expires"}</dt>
                  <dd classList={{ "is-bad": expired(), "is-warn": !expired() && days()! < 14 }}>
                    {fmtDate(acc().exp_date!)}
                    <small>{expired() ? ` · ${-days()!} days ago` : days() === 0 ? " · today" : ` · in ${days()} day${days() === 1 ? "" : "s"}`}</small>
                  </dd>
                </div>
              </Show>
              <Show when={acc().max_connections != null}>
                <div>
                  <dt>Connections</dt>
                  <dd classList={{ "is-warn": acc().active_connections != null && acc().active_connections! >= acc().max_connections! }}>
                    {acc().active_connections ?? 0} in use of {acc().max_connections}
                  </dd>
                </div>
              </Show>
              <Show when={r().channels != null}>
                <div>
                  <dt>Channels</dt>
                  <dd>{r().channels!.toLocaleString()}</dd>
                </div>
              </Show>
            </dl>
          )}
        </Show>
        <div class="testres__facts">
          <Show when={!a() && r().channels != null}><span>{r().channels!.toLocaleString()} channels found</span></Show>
          <Show when={r().epg_url}><span>Guide: <code>{redact(r().epg_url)}</code></span></Show>
          <Show when={props.filledLimit}><span class="testres__filled"><Icon name="check" /> Connection limit set to {props.filledLimit} from your account</span></Show>
        </div>
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

/**
 * People paste whatever their provider sent: a bare server, `…/player_api.php?username=…`, or the full
 * `get.php?username=…&password=…&type=m3u_plus` playlist link. Pull the login out of it.
 */
function parseProviderLink(input: string): { base: string; username?: string; password?: string; xtreamLink: boolean } | null {
  let raw = input.trim();
  if (!raw) return null;
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(raw)) raw = "http://" + raw;
  let u: URL;
  try {
    u = new URL(raw);
  } catch {
    return null;
  }
  const q = u.searchParams;
  const xtreamLink = /\/(get|player_api|xmltv|panel_api)\.php$/i.test(u.pathname) || (q.has("username") && q.has("password"));
  // Strip the Xtream endpoint (keep any path prefix the panel lives under).
  const path = u.pathname.replace(/\/(get|player_api|xmltv|panel_api)\.php$/i, "").replace(/\/+$/, "");
  return {
    base: `${u.protocol}//${u.host}${path}`,
    username: q.get("username") || (u.username ? decodeURIComponent(u.username) : undefined),
    password: q.get("password") || (u.password ? decodeURIComponent(u.password) : undefined),
    xtreamLink,
  };
}

function SourceForm(props: { source?: SourceDto; onDone: (s?: SourceDto, reload?: boolean) => void; extra?: JSX.Element }): JSX.Element {
  const src = props.source;
  const [kind, setKind] = createSignal<"xtream" | "m3u">(src?.kind === "m3u" ? "m3u" : "xtream");
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
  const [note, setNote] = createSignal<string>();
  const [result, setResult] = createSignal<SourceTest>();
  const [filled, setFilled] = createSignal<number | null>(null);

  // A test result only describes the settings it ran with.
  createEffect(on([kind, url, username, password], () => {
    setResult(undefined);
    setFilled(null);
  }, { defer: true }));

  /** Tidy the address; for Xtream, lift the login out of a pasted link. */
  const tidyUrl = () => {
    const p = parseProviderLink(url());
    if (!p) return;
    if (kind() === "xtream") {
      const got: string[] = [];
      if (p.username && p.username !== username()) {
        setUsername(p.username);
        got.push("username");
      }
      if (p.password && p.password !== password()) {
        setPassword(p.password);
        got.push("password");
      }
      if (p.base !== url()) setUrl(p.base);
      setNote(got.length ? `Picked up the ${got.join(" and ")} from the link.` : undefined);
    } else {
      if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url().trim())) setUrl("http://" + url().trim());
      setNote(
        p.xtreamLink && p.username && p.password
          ? "This is an Xtream link — switching to “Xtream Codes login” gives faster loading and account details."
          : undefined,
      );
    }
  };
  const switchToXtream = () => {
    setKind("xtream");
    tidyUrl();
  };

  const body = (): SourceBody => ({
    kind: kind(),
    name: name().trim() || hostOf(url()) || (kind() === "xtream" ? "IPTV" : "M3U playlist"),
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
    if (!/^https?:\/\/[^/\s]+/i.test(url().trim())) return "The address should look like http://server:port.";
    if (kind() === "xtream" && !username().trim()) return "Enter your Xtream username.";
    if (kind() === "xtream" && !password() && !src?.password_set) return "Enter your Xtream password.";
    if (epg().trim() && !/^https?:\/\//i.test(epg().trim())) return "The guide URL should start with http:// or https://.";
    return undefined;
  };

  const test = async () => {
    if (testing()) return;
    tidyUrl();
    const v = validate();
    if (v) return setError(v);
    setError(undefined);
    setTesting(true);
    setResult(undefined);
    try {
      // Editing without retyping the password: test the saved configuration instead.
      const r = src && !password() && src.password_set && kind() === "xtream" ? await iptv.testSource(src.id) : await iptv.testConfig(body());
      setResult(r);
      const lim = r.account?.max_connections;
      if (r.ok && lim && (maxConn().trim() === "" || (!src && Number(maxConn()) !== lim))) {
        setMaxConn(String(lim));
        setFilled(lim);
      }
    } catch (e) {
      setResult({ ok: false, message: e instanceof Error ? e.message : "Test failed" });
    } finally {
      setTesting(false);
    }
  };

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    tidyUrl();
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
        const reload = b.url !== src.url || (b.username ?? "") !== (src.username ?? "") || !!password() || (b.epg_url ?? "") !== (src.epg_url ?? "");
        props.onDone(await iptv.updateSource(src.id, patch), reload);
      } else {
        // The server starts the first channel + guide load itself; the row follows it.
        props.onDone(await iptv.createSource({ ...body(), enabled: true }));
      }
    } catch (err) {
      setError(err instanceof Error ? redact(err.message) : "Couldn't save the provider");
    } finally {
      setBusy(false);
    }
  };

  const failed = () => result() && !result()!.ok;

  return (
    <form class="subform iptv-form" onSubmit={submit} novalidate>
      <div class="segmented" role="group" aria-label="Provider type">
        <button type="button" class="segmented__btn" aria-pressed={kind() === "xtream"} onClick={switchToXtream}>Xtream Codes login</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "m3u"} onClick={() => { setKind("m3u"); setNote(undefined); }}>M3U playlist URL</button>
      </div>
      <div class="form-grid">
        <Field label="Name" placeholder={hostOf(url()) || (kind() === "xtream" ? "My provider" : "Playlist")} hint="Optional — shown in settings" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <Show
          when={kind() === "xtream"}
          fallback={
            <Field
              label="Playlist URL"
              placeholder="https://provider.example/get.php?username=…&type=m3u_plus"
              hint="The .m3u / .m3u8 link from your provider"
              value={url()}
              onInput={(e) => setUrl(e.currentTarget.value)}
              onBlur={tidyUrl}
              autocapitalize="off"
              spellcheck={false}
              inputmode="url"
              data-autofocus
            />
          }
        >
          <Field
            label="Server URL"
            placeholder="http://line.provider.example:8080"
            hint="Your provider's server and port. Pasting their full link works too."
            value={url()}
            onInput={(e) => setUrl(e.currentTarget.value)}
            onBlur={tidyUrl}
            onPaste={() => setTimeout(tidyUrl, 0)}
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
      </div>
      <Show when={note()}>
        <p class="iptv-note">
          <Icon name="info" /> {note()}
          <Show when={kind() === "m3u" && note()!.indexOf("Xtream") >= 0}>
            {" "}<button type="button" class="btn btn--sm" onClick={switchToXtream}><span>Use Xtream login</span></button>
          </Show>
        </p>
      </Show>
      <div class="form-grid">
        <Field
          label="Guide (EPG) URL"
          placeholder={kind() === "xtream" ? "Automatic from your provider" : src?.detected_epg_url ?? "From the playlist, if it has one"}
          hint={kind() === "xtream" ? "Optional — only to use a different XMLTV guide" : "Optional — an XMLTV link (.xml or .xml.gz)"}
          value={epg()}
          onInput={(e) => setEpg(e.currentTarget.value)}
          autocapitalize="off"
          spellcheck={false}
          inputmode="url"
        />
        <div class="field field--num">
          <label class="field__label" for="iptv-maxconn">Max connections</label>
          <input
            id="iptv-maxconn"
            class="field__input"
            type="number"
            min="0"
            inputmode="numeric"
            placeholder={kind() === "xtream" ? "Filled in by Test" : "Unknown"}
            value={maxConn()}
            onInput={(e) => {
              setMaxConn(e.currentTarget.value);
              setFilled(null);
            }}
          />
          <span class="field__hint">How many channels your plan can stream at once. Reel never opens more.</span>
        </div>
        <Select label="Refresh channels & guide" value={interval()} onChange={(e) => setIntervalV(e.currentTarget.value)} options={REFRESH_OPTS} />
      </div>
      <Show when={more()} fallback={<button type="button" class="btn btn--sm btn--quiet iptv-more" onClick={() => setMore(true)}><Icon name="plus" /> <span>More options</span></button>}>
        <div class="form-grid">
          <Field label="User agent" placeholder="Default" hint="Only if your provider requires a specific player" value={ua()} onInput={(e) => setUa(e.currentTarget.value)} autocapitalize="off" spellcheck={false} />
        </div>
      </Show>
      <Show when={testing()}>
        <div class="testres testres--pending" role="status"><span class="spinner spinner--xs" /> <span>Contacting {hostOf(url()) || "your provider"}…</span></div>
      </Show>
      <Show when={result()}>{(r) => <TestResult result={r()} filledLimit={filled()} />}</Show>
      <Show when={error()}><p class="form-error" role="alert"><Icon name="alert" /> {error()}</p></Show>
      <div class="actions">
        <button type="button" class="btn" classList={{ "btn--primary": !result() }} aria-disabled={testing() ? "true" : undefined} onClick={() => void test()}>
          <Icon name="activity" /> <span>{testing() ? "Testing…" : result() ? "Test again" : "Test connection"}</span>
        </button>
        <button type="submit" class="btn" classList={{ "btn--primary": !!result() && !failed() }} aria-disabled={busy() ? "true" : undefined}>
          <Icon name="check" />
          <span>{busy() ? "Saving…" : failed() ? "Save anyway" : src ? "Save changes" : "Save & load channels"}</span>
        </button>
        <button type="button" class="btn btn--quiet" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
      <Show when={props.extra}><div class="iptv-form__extra">{props.extra}</div></Show>
    </form>
  );
}

/** Provider errors can echo the request URL — never show credentials from it. */
function redact(msg: string | null | undefined): string {
  return (msg ?? "").replace(/((?:password|pass|pwd|token)=)[^&\s)]+/gi, "$1•••").replace(/\/\/([^/:@\s]+):([^@/\s]+)@/g, "//$1:•••@");
}

function hostOf(u: string): string {
  try {
    return new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(u.trim()) ? u.trim() : "http://" + u.trim()).hostname.replace(/^(www|line)\./, "");
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
  // Rows edit their item in place; this just re-runs the counts / filters.
  const update = () => mutate((l) => (l ? l.slice() : l));

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
              {(c) => <ChannelRow channel={c} onChange={() => update()} />}
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
  // Local copy: saving updates the row in place (replacing the list item would remount it and drop focus).
  const [cur, setCur] = createSignal(props.channel);
  const c = cur;
  const [open, setOpen] = createSignal(false);
  const [name, setName] = createSignal(c().name);
  const [num, setNum] = createSignal(c().number != null ? String(c().number) : "");
  const [group, setGroup] = createSignal(c().group ?? "");
  const [epg, setEpg] = createSignal(c().epg_manual ? c().epg_channel_id ?? "" : "");
  const [busy, setBusy] = createSignal(false);

  const save = async (patch: Parameters<typeof iptv.patchChannel>[1], local: Partial<AdminChannel>) => {
    try {
      await iptv.patchChannel(c().id, patch);
      Object.assign(props.channel, local);
      setCur({ ...props.channel });
      props.onChange(props.channel);
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
        <Show when={m().last_error}><span class="hub__err" title={redact(m().last_error)}><Icon name="alert" /></span></Show>
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

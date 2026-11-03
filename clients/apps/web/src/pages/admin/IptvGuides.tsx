import { createMemo, createSignal, For, onCleanup, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { ApiError } from "../../api/http";
import { iptv, type EpgSourceStat, type SourceDto } from "../../api/iptv";
import { toast } from "../../lib/toast";
import { timeAgo } from "../../components/AdminKit";

/** Server limit on external guide URLs per source. */
const MAX_GUIDES = 20;

export const epgshare = (code: string) => `https://epgshare01.online/epgshare01/epg_ripper_${code}.xml.gz`;

interface Suggestion {
  code: string;
  label: string;
  hint: string;
  /** Approximate download (gzip) size in MB — checked Nov 2026; they drift. */
  mb: number;
}

/** epgshare01 country/network files that exist (others 404). Most relevant for Arabic + international providers first. */
export const SUGGESTED_GUIDES: Suggestion[] = [
  { code: "AE1", label: "UAE", hint: "MBC, Rotana, OSN, Dubai, Abu Dhabi", mb: 2.4 },
  { code: "BEIN1", label: "beIN", hint: "beIN Sports & beIN channels", mb: 0.04 },
  { code: "ALJAZEERA1", label: "Al Jazeera", hint: "Al Jazeera network", mb: 0.01 },
  { code: "EG1", label: "Egypt", hint: "Egyptian channels", mb: 0.2 },
  { code: "SA1", label: "Saudi Arabia", hint: "Very small file", mb: 0.01 },
  { code: "TR1", label: "Turkey", hint: "Turkish channels", mb: 0.17 },
  { code: "UK1", label: "United Kingdom", hint: "BBC, ITV, Sky, …", mb: 2.8 },
  { code: "IE1", label: "Ireland", hint: "RTÉ, Virgin Media, …", mb: 2.5 },
  { code: "US2", label: "United States", hint: "Large — ~30 MB unpacked", mb: 6.5 },
  { code: "CA2", label: "Canada", hint: "Large — ~30 MB unpacked", mb: 6.5 },
  { code: "FR1", label: "France", hint: "TF1, Canal+, …", mb: 5.1 },
  { code: "BE2", label: "Belgium", hint: "Large", mb: 7.5 },
  { code: "DE1", label: "Germany", hint: "ARD, ZDF, Sky DE, …", mb: 3.4 },
  { code: "IT1", label: "Italy", hint: "RAI, Mediaset, …", mb: 1.6 },
  { code: "ES1", label: "Spain", hint: "Movistar, Atresmedia, …", mb: 3.3 },
  { code: "PT1", label: "Portugal", hint: "RTP, SIC, TVI, …", mb: 1.6 },
  { code: "NL1", label: "Netherlands", hint: "NPO, RTL, Ziggo, …", mb: 1.9 },
  { code: "PL1", label: "Poland", hint: "Large", mb: 8.9 },
  { code: "GR1", label: "Greece", hint: "ERT, Nova, …", mb: 1.6 },
  { code: "ZA1", label: "South Africa", hint: "DStv, SuperSport, SABC", mb: 0.18 },
  { code: "NG1", label: "Nigeria", hint: "Nigerian channels", mb: 0.14 },
  { code: "IN1", label: "India", hint: "Star, Zee, Sony, …", mb: 3.7 },
  { code: "AU1", label: "Australia", hint: "ABC, Seven, Nine, Foxtel, …", mb: 3.6 },
  { code: "ALL_SOURCES1", label: "Everything", hint: "Every country in one file — ~190 MB, slow to import", mb: 197 },
];

const sizeLabel = (mb: number) => (mb >= 100 ? `${Math.round(mb)} MB` : mb >= 1 ? `${mb.toFixed(1)} MB` : `${Math.max(1, Math.round(mb * 1000))} KB`);
const sizeClass = (mb: number) => (mb >= 100 ? "is-huge" : mb >= 5 ? "is-large" : "");

/** "epg_ripper_AE1.xml.gz" → "UAE (epgshare01)"; anything else → host + file name. */
export function guideLabel(url: string): { title: string; sub: string } {
  const m = /epgshare01[^/]*\/epgshare01\/epg_ripper_([A-Z0-9_]+)\.xml(?:\.gz)?$/i.exec(url);
  if (m) {
    const sug = SUGGESTED_GUIDES.find((x) => x.code === m[1]!.toUpperCase());
    return { title: sug ? `${sug.label} · ${sug.code}` : m[1]!, sub: "epgshare01" };
  }
  try {
    const u = new URL(url);
    const file = u.pathname.split("/").filter(Boolean).pop() ?? "";
    return { title: file || u.hostname, sub: u.hostname };
  } catch {
    return { title: url, sub: "" };
  }
}

/** Saved stats per external URL (positional on the server: provider first, then epg_urls in order). */
export function statsByUrl(src?: SourceDto): Map<string, EpgSourceStat> {
  const m = new Map<string, EpgSourceStat>();
  if (!src) return m;
  const ext = (src.epg_sources ?? []).filter((r) => !r.provider);
  (src.epg_urls ?? []).forEach((u, i) => ext[i] && m.set(u, ext[i]!));
  return m;
}

function StatLine(props: { stat?: EpgSourceStat; pending?: boolean }): JSX.Element {
  const st = () => props.stat;
  return (
    <Show
      when={st() && (st()!.last_at || st()!.last_error)}
      fallback={<span class="guide-row__stat is-muted">{props.pending ? "Not imported yet — save, then Refresh guide" : "Not imported yet"}</span>}
    >
      <span class="guide-row__stat" classList={{ "is-bad": !!st()!.last_error && !st()!.programmes, "is-ok": !st()!.last_error && st()!.programmes > 0 }}>
        <Show
          when={!st()!.last_error || st()!.programmes > 0}
          fallback={<><Icon name="alert" /> {friendlyGuideError(st()!.last_error!)}</>}
        >
          {st()!.programmes.toLocaleString()} programmes · {st()!.matched.toLocaleString()} channels matched
          <Show when={(st()!.available ?? 0) > st()!.matched}>
            <span class="guide-row__avail" title="Channels this guide also covers, but an earlier guide in the list supplies them">
              {" "}({st()!.available!.toLocaleString()} covered — move up to use it for more)
            </span>
          </Show>
          <Show when={st()!.channels}> · {st()!.channels.toLocaleString()} guide channels</Show>
        </Show>
        <Show when={st()!.last_at}><span class="guide-row__when">· {timeAgo(st()!.last_at!)}</span></Show>
      </span>
    </Show>
  );
}

/** Server parse errors → plain words (full text stays in the tooltip). */
export function friendlyGuideError(e: string): string {
  const s = e.replace(/^parse error:\s*/i, "");
  if (/no channels and no programmes/i.test(s)) return "Empty — this guide sent nothing";
  if (/none belong to your channels/i.test(s)) return "No programmes for your channels (names didn't match)";
  if (/none fall inside the 7-day window/i.test(s)) return "Programmes are outside the next 7 days (dates look wrong)";
  if (/timed? ?out/i.test(s)) return "Timed out downloading the guide";
  if (/40[134]|not found/i.test(s)) return "The guide URL wasn't found";
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/**
 * Ordered guide URLs for a source (order = precedence: for a channel several guides cover, the earlier
 * one wins). The provider's own guide is shown read-only above them and is never sent back.
 */
export function GuideListEditor(props: {
  kind: "xtream" | "m3u";
  source?: SourceDto;
  value: string[];
  onChange: (urls: string[]) => void;
}): JSX.Element {
  const [draft, setDraft] = createSignal("");
  const [err, setErr] = createSignal<string>();
  const [picker, setPicker] = createSignal(false);
  const saved = createMemo(() => statsByUrl(props.source));
  const provider = () => (props.source?.epg_sources ?? []).find((r) => r.provider);
  const savedSet = createMemo(() => new Set(props.source?.epg_urls ?? []));

  const add = (raw: string): boolean => {
    const u = raw.trim();
    if (!u) return false;
    if (!/^https?:\/\/[^/\s]+\/?/i.test(u)) {
      setErr("A guide URL starts with http:// or https://");
      return false;
    }
    if (props.value.indexOf(u) >= 0) {
      setErr("That guide is already in the list.");
      return false;
    }
    if (props.value.length >= MAX_GUIDES) {
      setErr(`At most ${MAX_GUIDES} guides.`);
      return false;
    }
    setErr(undefined);
    props.onChange(props.value.concat(u));
    return true;
  };
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= props.value.length) return;
    const l = props.value.slice();
    [l[i], l[j]] = [l[j]!, l[i]!];
    props.onChange(l);
  };
  const remove = (i: number) => props.onChange(props.value.filter((_u, k) => k !== i));

  return (
    <fieldset class="guides">
      <legend class="field__label guides__legend">
        TV guides <small>{props.value.length ? `${props.value.length} added · first wins` : "optional"}</small>
      </legend>
      <ol class="guides__list">
        <li class="guide-row guide-row--provider">
          <span class="guide-row__rank" aria-hidden="true"><Icon name="server" /></span>
          <div class="guide-row__main">
            <span class="guide-row__title">
              Provider guide <span class="pill pill--status">automatic</span>
            </span>
            <Show
              when={provider()}
              fallback={
                <span class="guide-row__stat is-muted">
                  {props.kind === "xtream" ? "Your provider's own guide (xmltv.php) is read first, when it has one." : "The playlist's x-tvg-url, if it has one, is read first."}
                </span>
              }
            >
              {(p) => (
                <>
                  <code class="guide-row__url" title={p().url}>{p().url}</code>
                  <span title={p().last_error ?? undefined}><StatLine stat={p()} /></span>
                </>
              )}
            </Show>
          </div>
        </li>
        <For each={props.value}>
          {(u, i) => {
            const lab = () => guideLabel(u);
            const st = () => saved().get(u);
            return (
              <li class="guide-row">
                <span class="guide-row__rank">{i() + 1}</span>
                <div class="guide-row__main">
                  <span class="guide-row__title">
                    {lab().title} <small>{lab().sub}</small>
                  </span>
                  <code class="guide-row__url" title={u}>{u}</code>
                  <span title={st()?.last_error ?? undefined}>
                    <StatLine stat={st()} pending={!savedSet().has(u)} />
                  </span>
                </div>
                <div class="guide-row__actions">
                  <button type="button" class="iconbtn iconbtn--sm" aria-label={`Move ${lab().title} up`} disabled={i() === 0} onClick={() => move(i(), -1)}>
                    <span class="guide-row__arrow" aria-hidden="true">↑</span>
                  </button>
                  <button type="button" class="iconbtn iconbtn--sm" aria-label={`Move ${lab().title} down`} disabled={i() === props.value.length - 1} onClick={() => move(i(), 1)}>
                    <span class="guide-row__arrow" aria-hidden="true">↓</span>
                  </button>
                  <button type="button" class="iconbtn iconbtn--sm guide-row__remove" aria-label={`Remove ${lab().title}`} onClick={() => remove(i())}>
                    <Icon name="x" />
                  </button>
                </div>
              </li>
            );
          }}
        </For>
      </ol>

      <div class="guides__add">
        <input
          class="field__input field__input--mono"
          type="url"
          inputmode="url"
          placeholder="https://… .xml or .xml.gz"
          value={draft()}
          onInput={(e) => {
            setDraft(e.currentTarget.value);
            setErr(undefined);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            e.preventDefault(); // don't submit the provider form
            if (add(draft())) setDraft("");
          }}
          autocapitalize="off"
          spellcheck={false}
          aria-label="Guide URL to add"
        />
        <button type="button" class="btn btn--sm" onClick={() => add(draft()) && setDraft("")}><Icon name="plus" /> <span>Add guide</span></button>
        <button type="button" class="btn btn--sm btn--quiet" aria-expanded={picker()} onClick={() => setPicker((v) => !v)}>
          <Icon name="compass" /> <span>{picker() ? "Hide suggestions" : "Suggested guides"}</span>
        </button>
      </div>
      <Show when={err()}><p class="form-error" role="alert"><Icon name="alert" /> {err()}</p></Show>

      <Show when={picker()}>
        <div class="guide-picks">
          <p class="guide-picks__lede">
            Free country guides from <strong>epgshare01</strong>. Add the countries your channels come from — Reel matches them to your channels by
            name. Big files take longer to import.
          </p>
          <div class="guide-picks__grid">
            <For each={SUGGESTED_GUIDES}>
              {(g) => {
                const url = epgshare(g.code);
                const added = () => props.value.indexOf(url) >= 0;
                return (
                  <button
                    type="button"
                    class="guide-pick"
                    classList={{ "is-added": added(), [sizeClass(g.mb)]: !!sizeClass(g.mb) }}
                    aria-pressed={added()}
                    disabled={added()}
                    onClick={() => add(url)}
                  >
                    <span class="guide-pick__top">
                      <strong>{g.label}</strong>
                      <code>{g.code}</code>
                    </span>
                    <span class="guide-pick__hint">{g.hint}</span>
                    <span class="guide-pick__size">
                      <Show when={added()} fallback={<><Icon name="download" /> {sizeLabel(g.mb)}{g.mb >= 100 ? " · very large" : g.mb >= 5 ? " · large" : ""}</>}>
                        <Icon name="check" /> Added
                      </Show>
                    </span>
                  </button>
                );
              }}
            </For>
          </div>
        </div>
      </Show>
    </fieldset>
  );
}

interface EpgJob {
  prev: number | null;
  started: number;
  seen: boolean;
  before: { programmes: number; matched: number };
}

/**
 * The source's TV guide at a glance — totals, each guide's result — and the "Refresh guide" action
 * (guide import only, no channel reload) with progress and what changed.
 */
export function GuidePanel(props: { source: SourceDto; onChange: (s: SourceDto) => void; dirty?: boolean; onClean?: () => void; onRunning?: (v: boolean) => void }): JSX.Element {
  const s = () => props.source;
  const [open, setOpen] = createSignal(false);
  const [job, setJob] = createSignal<EpgJob>();
  const [elapsed, setElapsed] = createSignal(0);
  const [done, setDone] = createSignal<{ ok: boolean; text: string }>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let ticker: ReturnType<typeof setInterval> | undefined;
  onCleanup(() => {
    if (timer) clearTimeout(timer);
    if (ticker) clearInterval(ticker);
  });

  const guides = () => s().epg_sources ?? [];
  const ok = () => guides().filter((g) => !g.last_error || g.programmes > 0).filter((g) => g.programmes > 0).length;
  const failing = () => guides().filter((g) => g.last_error && !g.programmes).length;
  const mode = () => s().epg_mode ?? "none";

  const stop = (r: { ok: boolean; text: string }) => {
    setJob(undefined);
    props.onRunning?.(false);
    if (ticker) clearInterval(ticker);
    setDone(r);
  };
  const poll = async (j: EpgJob) => {
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
    if (cur.refreshing) j.seen = true;
    else if (cur.last_epg_at != null && cur.last_epg_at !== j.prev) {
      const dp = (cur.epg_programme_count ?? 0) - j.before.programmes;
      const dm = (cur.epg_matched_channels ?? 0) - j.before.matched;
      const sign = (n: number) => (n > 0 ? ` (+${n.toLocaleString()})` : n < 0 ? ` (${n.toLocaleString()})` : "");
      stop({
        ok: (cur.epg_programme_count ?? 0) > 0,
        text: `${(cur.epg_programme_count ?? 0).toLocaleString()} programmes${sign(dp)} · ${(cur.epg_matched_channels ?? 0).toLocaleString()} channels with a guide${sign(dm)}`,
      });
      return;
    } else if (j.seen || nowS() - j.started > 20) {
      stop({ ok: false, text: cur.last_error ? cur.last_error : "The guide import ended without new data." });
      return;
    }
    if (nowS() - j.started > 20 * 60) return stop({ ok: false, text: "Still importing after 20 minutes — it carries on in the background." });
    timer = setTimeout(() => void poll(j), 2000);
  };
  const run = async () => {
    if (job() || s().refreshing) return;
    setDone(undefined);
    const j: EpgJob = {
      prev: s().last_epg_at ?? null,
      started: nowS(),
      seen: false,
      before: { programmes: s().epg_programme_count ?? 0, matched: s().epg_matched_channels ?? 0 },
    };
    try {
      await iptv.refreshEpg(s().id);
      j.seen = true;
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 409)) {
        toast(e instanceof Error ? e.message : "Couldn't start the guide refresh", "error");
        return;
      }
      j.seen = true; // one is already running: follow it
    }
    props.onClean?.();
    props.onRunning?.(true);
    setJob(j);
    setElapsed(0);
    if (ticker) clearInterval(ticker);
    ticker = setInterval(() => setElapsed(nowS() - j.started), 1000);
    timer = setTimeout(() => void poll(j), 1500);
  };
  const clock = () => `${Math.floor(elapsed() / 60)}:${String(elapsed() % 60).padStart(2, "0")}`;

  return (
    <section class="guidepanel" aria-label="TV guide">
      <div class="guidepanel__head">
        <div class="guidepanel__sum">
          <strong class="guidepanel__title">
            <Icon name="grid" /> TV guide
            <span class={`pill pill--status guidepanel__mode guidepanel__mode--${mode()}`}>
              {mode() === "xmltv" ? "Full guide" : mode() === "per_channel" ? "Per channel (provider)" : "No guide"}
            </span>
          </strong>
          <span class="guidepanel__totals">
            <span><b>{(s().epg_programme_count ?? 0).toLocaleString()}</b> programmes</span>
            <span><b>{(s().epg_matched_channels ?? 0).toLocaleString()}</b> of {s().channel_count.toLocaleString()} channels matched</span>
            <Show when={s().last_epg_at}><span>updated {timeAgo(s().last_epg_at!)}</span></Show>
          </span>
          <Show when={s().epg_note}><span class="form-warn"><Icon name="info" /> {s().epg_note}</span></Show>
        </div>
        <div class="guidepanel__actions">
          <Show when={guides().length}>
            <button type="button" class="btn btn--sm btn--quiet" aria-expanded={open()} onClick={() => setOpen((v) => !v)}>
              <span>{guides().length} guide{guides().length === 1 ? "" : "s"}</span>
              <Show when={failing()}><span class="guidepanel__bad">{failing()} not working</span></Show>
              <Icon name={open() ? "x" : "down"} />
            </button>
          </Show>
          <button
            type="button"
            class="btn btn--sm"
            classList={{ "btn--primary": !!props.dirty }}
            aria-disabled={job() || s().refreshing ? "true" : undefined}
            onClick={() => void run()}
          >
            <Icon name="retry" /> <span>{job() ? "Refreshing guide…" : "Refresh guide"}</span>
          </button>
        </div>
      </div>

      <Show when={props.dirty && !job()}>
        <p class="guidepanel__dirty"><Icon name="info" /> Guide list saved. Press <strong>Refresh guide</strong> to import it — this doesn't reload your channels.</p>
      </Show>

      <Show when={job()}>
        <div class="loadjob" role="status" aria-live="polite">
          <div class="loadjob__head">
            <strong>Importing {Math.max(1, guides().length)} guide{guides().length === 1 ? "" : "s"}</strong>
            <span class="loadjob__clock">{clock()}</span>
          </div>
          <div class="loadjob__bar" aria-hidden="true"><i /></div>
          <p class="loadjob__hint">
            {elapsed() < 30 ? "Downloading and reading the guides…" : "Matching programmes to your channels. Large guides take a few minutes — you can leave this page."}
          </p>
        </div>
      </Show>

      <Show when={done()}>
        {(d) => (
          <div class="testres" classList={{ "is-ok": d().ok, "is-bad": !d().ok }} role="status">
            <Icon name={d().ok ? "check" : "alert"} />
            <div class="testres__body">
              <strong>{d().ok ? "Guide refreshed" : "Guide refresh didn't add anything"}</strong>
              <div class="testres__facts"><span>{d().text}</span></div>
            </div>
            <button type="button" class="iconbtn iconbtn--sm" aria-label="Dismiss" onClick={() => setDone(undefined)}><Icon name="x" /></button>
          </div>
        )}
      </Show>

      <Show when={open()}>
        <ol class="guides__list guides__list--read">
          <For each={guides()}>
            {(g, i) => {
              const lab = () => (g.provider ? { title: "Provider guide", sub: "automatic" } : guideLabel((s().epg_urls ?? [])[guides().filter((x) => x.provider).length ? i() - 1 : i()] ?? g.url));
              return (
                <li class="guide-row" classList={{ "guide-row--provider": g.provider }}>
                  <span class="guide-row__rank">{g.provider ? <Icon name="server" /> : guides().some((x) => x.provider) ? i() : i() + 1}</span>
                  <div class="guide-row__main">
                    <span class="guide-row__title">{lab().title} <small>{lab().sub}</small></span>
                    <code class="guide-row__url" title={g.url}>{g.url}</code>
                    <span title={g.last_error ?? undefined}><StatLine stat={g} /></span>
                  </div>
                </li>
              );
            }}
          </For>
        </ol>
        <Show when={ok() === 0 && guides().length}>
          <p class="guidepanel__tip">No guide delivered programmes yet. Edit the provider to add country guides that match your channels.</p>
        </Show>
      </Show>
    </section>
  );
}

const nowS = () => Math.floor(Date.now() / 1000);

import { createEffect, createMemo, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { Icon, onBack, onRemoteKey } from "@reel/ui";
import "../styles/live.css";
import { iptv, type IptvChannel, type Program } from "../api/iptv";
import { href } from "../lib/mode";
import { isAdmin } from "../lib/auth";
import { ErrorState } from "../components/States";
import { ChannelLogo } from "../components/live/ChannelLogo";
import { Filters } from "../components/live/Filters";
import {
  channelById, channelByNumber, channelsError, clock, filter, filterKey, filteredList, loadAll, loadState, fmtDay, fmtRange, fmtTime, loadChannels, nowSec, progressOf, setFilter, getBrowsingChannel, setBrowsingChannel,
} from "../components/live/store";

// Geometry (rem) — converted to px with the live root font size (TV scales rem with the viewport).
const HALF_REM = 9.5; // one half-hour column
const ROW_REM = 4.4;
const CH_REM = 15;
const HEAD_REM = 2.6;

const HALF = 1800;
const CHUNK = 6 * 3600; // EPG fetch granularity (time)
const BATCH = 60; // channels per EPG request
const INITIAL_SPAN = 12 * 3600;
const MAX_SPAN = 72 * 3600;
const OVERSCAN_ROWS = 4;

type CellKind = "prog" | "gap" | "loading";
interface Cell {
  key: string;
  ch: number;
  kind: CellKind;
  start: number;
  stop: number;
  prog?: Program;
}

function rootPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

export default function LiveGuide(): JSX.Element {
  onMount(() => {
    void loadChannels();
    document.documentElement.classList.add("is-live-page");
    onCleanup(() => document.documentElement.classList.remove("is-live-page"));
  });
  return (
    <div class="guide-page">
      <Switch>
        <Match when={loadState() === "error"}>
          <div class="page"><ErrorState error={channelsError()} onRetry={() => void loadChannels(true)} title="Couldn't load the guide" /></div>
        </Match>
        <Match when={loadState() === "loading"}>
          <div class="guide guide--loading" aria-busy="true"><span class="spinner" /></div>
        </Match>
        <Match when={loadState() === "empty"}>
          <div class="page">
            <div class="state">
              <p class="eyebrow eyebrow--accent">Guide</p>
              <h1 class="state__title">No channels yet</h1>
              <p class="state__body">The guide fills in once Live TV has channels.</p>
              <a class="btn btn--primary" href={href("/live")} data-autofocus><Icon name="live" /> <span>Go to Live TV</span></a>
            </div>
          </div>
        </Match>
        <Match when={loadState() === "ready"}>
          <Grid />
        </Match>
      </Switch>
    </div>
  );
}

function Grid(): JSX.Element {
  const navigate = useNavigate();
  let scroller!: HTMLDivElement;
  const [px, setPx] = createSignal(16);
  const [view, setView] = createSignal({ left: 0, top: 0, width: 1200, height: 700 });
  const [span, setSpan] = createSignal(INITIAL_SPAN);
  const [rev, setRev] = createSignal(0);
  const [detail, setDetail] = createSignal<Cell>();
  const [focused, setFocused] = createSignal<Cell>();
  const [digits, setDigits] = createSignal("");

  // Window origin: an hour before the current half-hour, fixed for the life of the page.
  const origin = Math.floor(nowSec() / HALF) * HALF - 3600;
  const halfPx = () => HALF_REM * px();
  const pps = () => halfPx() / HALF;
  const rowPx = () => ROW_REM * px();
  const chPx = () => CH_REM * px();
  const headPx = () => HEAD_REM * px();
  const xOf = (t: number) => chPx() + (t - origin) * pps();

  const rows = createMemo(() => filteredList() ?? []);
  const rowIndex = createMemo(() => {
    const m = new Map<number, number>();
    rows().forEach((c, i) => m.set(c.id, i));
    return m;
  });

  // ---- data -----------------------------------------------------------------------
  const progs = new Map<number, Program[]>();
  const loaded = new Map<number, Set<number>>();
  const inflight = new Set<string>();
  const nChunks = () => Math.ceil(span() / CHUNK);
  const isLoaded = (ch: number, chunk: number) => !!loaded.get(ch)?.has(chunk);

  /** Programmes received so far (any channel) — 0 means the provider has no guide yet. */
  let progTotal = 0;
  const merge = (ch: number, list: Program[]) => {
    const cur = progs.get(ch) ?? [];
    const seen = new Set(cur.map((p) => p.start));
    let changed = false;
    list.forEach((p) => {
      if (!seen.has(p.start)) {
        cur.push(p);
        seen.add(p.start);
        changed = true;
      }
    });
    if (changed) cur.sort((a, b) => a.start - b.start);
    progs.set(ch, cur);
    progTotal += list.length;
  };

  let fetchTimer: ReturnType<typeof setTimeout> | undefined;
  const scheduleFetch = () => {
    if (fetchTimer) clearTimeout(fetchTimer);
    fetchTimer = setTimeout(fetchVisible, 90);
  };
  const fetchVisible = () => {
    const [r0, r1] = rowRange();
    const ids: number[] = [];
    const rl = rows();
    // Prefetch a little beyond the viewport so D-pad scrolling lands on loaded rows.
    for (let i = Math.max(0, r0 - 6); i < Math.min(rl.length, r1 + 10); i++) ids.push(rl[i]!.id);
    const [t0, t1] = timeRange();
    const c0 = Math.max(0, Math.floor((t0 - origin) / CHUNK));
    const c1 = Math.min(nChunks() - 1, Math.floor((t1 - origin) / CHUNK));
    for (let chunk = c0; chunk <= c1; chunk++) {
      const need = ids.filter((id) => !isLoaded(id, chunk) && !inflight.has(`${id}:${chunk}`));
      for (let i = 0; i < need.length; i += BATCH) {
        const batchIds = need.slice(i, i + BATCH);
        batchIds.forEach((id) => inflight.add(`${id}:${chunk}`));
        const from = origin + chunk * CHUNK;
        iptv
          .epg({ from, to: from + CHUNK, channels: batchIds })
          .then((g) => {
            g.channels.forEach((c) => merge(c.channel_id, c.programs));
            batchIds.forEach((id) => {
              let s = loaded.get(id);
              if (!s) loaded.set(id, (s = new Set()));
              s.add(chunk);
            });
            setRev((v) => v + 1);
          })
          .catch(() => undefined)
          .finally(() => batchIds.forEach((id) => inflight.delete(`${id}:${chunk}`)));
      }
    }
  };

  // ---- row model ----------------------------------------------------------------------
  const cellCache = new Map<string, Cell>();
  const mk = (ch: number, kind: CellKind, start: number, stop: number, prog?: Program): Cell => {
    const key = `${ch}:${kind}:${start}:${stop}`;
    let c = cellCache.get(key);
    if (!c) cellCache.set(key, (c = { key, ch, kind, start, stop, prog }));
    return c;
  };
  const pushGap = (out: Cell[], ch: number, a: number, b: number, kind: "gap" | "loading") => {
    // Split long gaps on 2-hour boundaries so cells stay a sensible size.
    let t = a;
    while (t < b) {
      const edge = origin + (Math.floor((t - origin) / 7200) + 1) * 7200;
      const e = Math.min(b, edge);
      out.push(mk(ch, kind, t, e));
      t = e;
    }
  };
  const modelCache = new Map<number, { rev: number; span: number; cells: Cell[] }>();
  /** Every cell of a row across the loaded window: programmes, gaps and loading placeholders. */
  const rowModel = (ch: number): Cell[] => {
    const r = rev();
    const sp = span();
    const hit = modelCache.get(ch);
    if (hit && hit.rev === r && hit.span === sp) return hit.cells;
    const out: Cell[] = [];
    const list = progs.get(ch) ?? [];
    let t = origin;
    const end = origin + sp;
    let pi = 0;
    for (let chunk = 0; chunk * CHUNK < sp; chunk++) {
      const cs = origin + chunk * CHUNK;
      const ce = Math.min(end, cs + CHUNK);
      if (t >= ce) continue;
      if (!isLoaded(ch, chunk)) {
        pushGap(out, ch, Math.max(t, cs), ce, "loading");
        t = ce;
        continue;
      }
      while (pi < list.length && list[pi]!.stop <= t) pi++;
      while (pi < list.length && list[pi]!.start < ce) {
        const p = list[pi]!;
        if (p.start > t) pushGap(out, ch, t, p.start, "gap");
        out.push(mk(ch, "prog", p.start, p.stop, p));
        t = Math.max(t, p.stop);
        pi++;
      }
      if (t < ce) {
        pushGap(out, ch, t, ce, "gap");
        t = ce;
      }
    }
    modelCache.set(ch, { rev: r, span: sp, cells: out });
    if (modelCache.size > 300) modelCache.clear();
    return out;
  };

  // ---- viewport ---------------------------------------------------------------------
  const rowRange = createMemo(() => {
    const v = view();
    const h = rowPx();
    const top = Math.max(0, v.top);
    const r0 = Math.max(0, Math.floor(top / h) - OVERSCAN_ROWS);
    const r1 = Math.min(rows().length, Math.ceil((top + v.height) / h) + OVERSCAN_ROWS);
    return [r0, r1] as const;
  });
  const timeRange = createMemo(() => {
    const v = view();
    const t0 = origin + Math.max(0, v.left) / pps();
    const t1 = origin + (v.left + v.width - chPx()) / pps();
    return [t0, t1] as const;
  });
  /** Cells drawn: the visible window plus an hour either side. */
  const drawRange = createMemo(() => {
    const [t0, t1] = timeRange();
    return [t0 - 3600, t1 + 3600] as const;
  });

  // Stable row objects per channel so <For> keeps DOM nodes (and focus) while scrolling.
  type RowRef = { c: IptvChannel; i: () => number; setI: (n: number) => void };
  const rowRefs = new Map<number, RowRef>();
  const visibleRows = createMemo(() => {
    const [r0, r1] = rowRange();
    const rl = rows();
    const out: RowRef[] = [];
    for (let i = r0; i < r1; i++) {
      const c = rl[i]!;
      let r = rowRefs.get(c.id);
      if (!r || r.c !== c) {
        const [gi, si] = createSignal(i);
        r = { c, i: gi, setI: si };
        rowRefs.set(c.id, r);
      } else r.setI(i);
      out.push(r);
    }
    if (rowRefs.size > 300) {
      const keep = new Set(out.map((r) => r.c.id));
      rowRefs.forEach((_v, k) => !keep.has(k) && rowRefs.delete(k));
    }
    return out;
  });

  const measure = () => {
    if (!scroller) return;
    setView({ left: scroller.scrollLeft, top: scroller.scrollTop, width: scroller.clientWidth, height: scroller.clientHeight - headPx() });
  };
  let raf = 0;
  const onScroll = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      measure();
      // Near the right edge: extend the window (lazy horizontal loading).
      const [, t1] = timeRange();
      if (t1 > origin + span() - 3 * 3600 && span() < MAX_SPAN) setSpan((s) => Math.min(MAX_SPAN, s + CHUNK));
    });
  };
  createEffect(on([rowRange, timeRange, span, rows], scheduleFetch));

  /** Everything in view has loaded and none of it has a programme. */
  const emptyView = createMemo(() => {
    rev();
    const rl = rows();
    if (!rl.length) return false;
    const [r0, r1] = rowRange();
    const [t0, t1] = timeRange();
    const c0 = Math.max(0, Math.floor((t0 - origin) / CHUNK));
    const c1 = Math.min(nChunks() - 1, Math.floor((t1 - origin) / CHUNK));
    for (let i = r0; i < r1; i++) {
      const id = rl[i]!.id;
      for (let c = c0; c <= c1; c++) if (!isLoaded(id, c)) return false;
      if ((progs.get(id) ?? []).length) return false;
    }
    return true;
  });
  const providerHasNoGuide = () => (rev(), progTotal === 0);

  // ---- focus model --------------------------------------------------------------------
  /** Time cursor: vertical moves keep it, so short programmes don't make focus drift. */
  let cursorT = nowSec();
  let gridFocus = false;

  const cellAt = (ch: number, t: number): Cell | undefined => {
    const m = rowModel(ch);
    let best: Cell | undefined;
    for (const c of m) {
      if (c.start <= t && c.stop > t) return c;
      if (c.start > t) return best ?? c;
      best = c;
    }
    return best;
  };

  /** `lateral`: a left/right move (align the cell); vertical moves only scroll when it's off screen. */
  const ensureVisible = (c: Cell, row: number, lateral = false) => {
    const L = scroller.scrollLeft;
    const W = scroller.clientWidth;
    const x0 = xOf(Math.max(origin, c.start));
    const x1 = xOf(c.stop);
    const visL = L + chPx();
    let nl = L;
    const pad = halfPx() * 0.5;
    if (lateral) {
      // Reveal the whole cell when it fits; long programmes align their start instead.
      if (x1 > L + W) nl = Math.min(x1 - W + pad, x0 - chPx() - pad);
      else if (x0 < visL) nl = x0 - chPx() - pad;
    } else if (x1 < visL + halfPx() * 0.5) nl = x0 - chPx();
    else if (x0 > L + W - halfPx() * 0.5) nl = x0 - chPx() - halfPx();
    const T = scroller.scrollTop;
    const H = scroller.clientHeight - headPx();
    const y0 = row * rowPx();
    let nt = T;
    if (y0 < T + rowPx()) nt = y0 - rowPx();
    else if (y0 + rowPx() > T + H - rowPx()) nt = y0 + 2 * rowPx() - H;
    nl = Math.max(0, nl);
    nt = Math.max(0, nt);
    if (nl !== L) scroller.scrollLeft = nl;
    if (nt !== T) scroller.scrollTop = nt;
    measure();
  };

  const focusCell = (c: Cell, lateral = false) => {
    const row = rowIndex().get(c.ch);
    if (row == null) return;
    // Grow the window first so the canvas is wide enough to scroll the cell into view.
    const need = Math.min(MAX_SPAN, Math.ceil((c.stop - origin + 3 * 3600) / CHUNK) * CHUNK);
    if (need > span()) setSpan(need);
    ensureVisible(c, row, lateral);
    setFocused(c);
    setBrowsingChannel(c.ch);
    const el = scroller.querySelector<HTMLElement>(`[data-fid="${cssEsc(c.key)}"]`);
    if (el) el.focus({ preventScroll: true });
    else requestAnimationFrame(() => scroller.querySelector<HTMLElement>(`[data-fid="${cssEsc(c.key)}"]`)?.focus({ preventScroll: true }));
  };

  const focusRowAt = (row: number, t = cursorT) => {
    const ch = rows()[row];
    if (!ch) return;
    const c = cellAt(ch.id, t);
    if (c) focusCell(c);
  };

  const focusChannelCell = (row: number) => {
    const ch = rows()[row];
    if (!ch) return;
    const y0 = row * rowPx();
    const T = scroller.scrollTop;
    const H = scroller.clientHeight - headPx();
    if (y0 < T + rowPx()) scroller.scrollTop = Math.max(0, y0 - rowPx());
    else if (y0 + rowPx() > T + H - rowPx()) scroller.scrollTop = y0 + 2 * rowPx() - H;
    measure();
    scroller.querySelector<HTMLElement>(`[data-fid="gch-${ch.id}"]`)?.focus({ preventScroll: true });
  };

  // Data arrived and the focused placeholder vanished → move focus to what replaced it.
  createEffect(on([rev, span], () => {
    const f = focused();
    if (!f || !gridFocus) return;
    requestAnimationFrame(() => {
      const a = document.activeElement;
      if (a && a !== document.body && scroller.contains(a)) return;
      const c = cellAt(f.ch, Math.max(f.start, cursorT));
      if (c) focusCell(c, f.kind === "loading");
    });
  }, { defer: true }));

  const move = (dir: "up" | "down" | "left" | "right"): boolean => {
    const a = document.activeElement as HTMLElement | null;
    if (!a || !scroller.contains(a)) return false;
    const chCell = a.getAttribute("data-guide-ch");
    if (chCell) {
      const row = rowIndex().get(Number(chCell)) ?? 0;
      if (dir === "up") {
        if (row === 0) return false;
        focusChannelCell(row - 1);
        return true;
      }
      if (dir === "down") {
        focusChannelCell(Math.min(rows().length - 1, row + 1));
        return true;
      }
      if (dir === "right") {
        cursorT = Math.max(timeRange()[0], Math.min(nowSec(), timeRange()[1]));
        focusRowAt(row);
        return true;
      }
      return false;
    }
    const f = focused();
    if (!f || a.getAttribute("data-fid") !== f.key) return false;
    const row = rowIndex().get(f.ch);
    if (row == null) return false;
    if (dir === "up" || dir === "down") {
      const nr = row + (dir === "up" ? -1 : 1);
      if (nr < 0) return false; // up into the filters
      if (nr >= rows().length) return true;
      focusRowAt(nr);
      return true;
    }
    const m = rowModel(f.ch);
    const i = m.indexOf(f);
    if (dir === "right") {
      const n = m[i + 1];
      if (!n) {
        if (span() < MAX_SPAN) {
          setSpan((s) => Math.min(MAX_SPAN, s + CHUNK));
          const nn = rowModel(f.ch)[i + 1];
          if (nn) {
            cursorT = nn.start;
            focusCell(nn, true);
          }
        }
        return true;
      }
      cursorT = n.start;
      focusCell(n, true);
      return true;
    }
    const p = m[i - 1];
    if (!p || p.stop <= origin) return false; // to the channel cell
    cursorT = Math.max(origin, p.start);
    focusCell(p, true);
    return true;
  };

  // Number entry jumps to a channel row.
  let digitTimer: ReturnType<typeof setTimeout> | undefined;
  const jumpToNumber = async () => {
    const n = Number(digits());
    setDigits("");
    let c = channelByNumber(n);
    if (!c || rowIndex().get(c.id) == null) {
      // Not in this group: switch to every channel (needs the full list).
      try {
        await loadAll();
      } catch {
        return;
      }
      c = channelByNumber(n);
      if (!c) return;
      if (rowIndex().get(c.id) == null) setFilter({ kind: "all" });
    }
    const row = rowIndex().get(c.id);
    if (row == null) return;
    // Land on what's on now.
    cursorT = nowSec();
    scroller.scrollTop = Math.max(0, row * rowPx() - rowPx());
    measure();
    focusRowAt(row);
  };

  const goNow = () => {
    scroller.scrollLeft = Math.max(0, (Math.floor(nowSec() / HALF) * HALF - origin) * pps());
    measure();
    cursorT = nowSec();
    const [r0] = rowRange();
    focusRowAt(Math.min(rows().length - 1, Math.max(0, Math.ceil(scroller.scrollTop / rowPx()))) || r0);
  };

  onMount(() => {
    setPx(rootPx());
    // Lazy routes render detached until ready: wait for real layout before positioning.
    let tries = 0;
    const place = () => {
      if ((!scroller.isConnected || !scroller.clientWidth) && tries++ < 60) return void requestAnimationFrame(place);
      setPx(rootPx());
      scroller.scrollLeft = (Math.floor(nowSec() / HALF) * HALF - origin) * pps();
      measure();
      focusRowAt(rowIndex().get(getBrowsingChannel() ?? -1) ?? 0, nowSec());
    };
    requestAnimationFrame(place);
    const onResize = () => {
      setPx(rootPx());
      measure();
    };
    window.addEventListener("resize", onResize);
    const offKeys = onRemoteKey((e) => {
      if (detail()) return false;
      const k = e.key;
      if (k === "up" || k === "down" || k === "left" || k === "right") return move(k);
      if (k === "channelUp" || k === "channelDown") {
        const f = focused();
        const row = f ? rowIndex().get(f.ch) ?? 0 : 0;
        const page = Math.max(1, Math.floor(view().height / rowPx()) - 1);
        focusRowAt(Math.max(0, Math.min(rows().length - 1, row + (k === "channelUp" ? -page : page))));
        return true;
      }
      if (k === "digit") {
        setDigits((d) => (d.length >= 4 ? "" : d) + String(e.digit ?? 0));
        if (digitTimer) clearTimeout(digitTimer);
        digitTimer = setTimeout(() => void jumpToNumber(), 1200);
        return true;
      }
      return false;
    });
    const offBack = onBack(() => {
      if (digits()) {
        setDigits("");
        return true;
      }
      return false;
    });
    onCleanup(() => {
      window.removeEventListener("resize", onResize);
      offKeys();
      offBack();
      if (raf) cancelAnimationFrame(raf);
      if (fetchTimer) clearTimeout(fetchTimer);
      if (digitTimer) clearTimeout(digitTimer);
    });
  });

  // Filter changed → top of the list, focus what's on now.
  createEffect(on(() => filterKey(filter()), () => {
    if (!scroller) return;
    scroller.scrollTop = 0;
    measure();
  }, { defer: true }));

  const watch = (ch: number) => navigate(`/live?ch=${ch}`);

  const t = () => clock();
  const nowX = () => xOf(t());
  const ticks = createMemo(() => {
    const out: number[] = [];
    for (let x = origin; x < origin + span(); x += HALF) out.push(x);
    return out;
  });
  const canvasW = () => chPx() + span() * pps();
  const canvasH = () => headPx() + rows().length * rowPx();
  const dayAt = () => fmtDay(Math.max(origin, timeRange()[0]));

  return (
    <div class="guide">
      <header class="guide__head">
        <div class="guide__titles">
          <p class="eyebrow eyebrow--accent">Live TV</p>
          <h1 class="live__title">Guide</h1>
        </div>
        <div class="guide__actions">
          <button type="button" class="btn btn--sm" onClick={goNow}><Icon name="clock" /> <span>Now</span></button>
          <a class="btn btn--sm" href={href("/live")}><Icon name="live" /> <span>Channels</span></a>
        </div>
      </header>
      <Filters class="guide__filters" label={false} />
      <Preview cell={focused()} />

      <div
        ref={scroller}
        class="guide__grid"
        style={{ "--ch-w": `${chPx()}px` }}
        onScroll={onScroll}
        onFocusIn={() => (gridFocus = true)}
        onFocusOut={(e) => {
          const n = e.relatedTarget as Node | null;
          if (n && !scroller.contains(n)) gridFocus = false;
        }}
        data-nav-group
      >
        <div class="guide__canvas" style={{ width: `${canvasW()}px`, height: `${canvasH()}px` }}>
          <div class="guide__ruler" style={{ width: `${canvasW()}px`, height: `${headPx()}px` }}>
            <div class="guide__corner" style={{ width: `${chPx()}px` }}>
              <span class="guide__corner-day">{dayAt()}</span>
            </div>
            <For each={ticks()}>
              {(x) => (
                <span class="guide__tick" classList={{ "is-hour": new Date(x * 1000).getMinutes() === 0 }} style={{ left: `${xOf(x)}px`, width: `${halfPx()}px` }}>
                  {fmtTime(x)}
                </span>
              )}
            </For>
            <Show when={t() >= origin && t() < origin + span()}>
              <span class="guide__nowtick" style={{ left: `${nowX()}px` }}>{fmtTime(t())}</span>
            </Show>
          </div>

          <For each={visibleRows()}>
            {(r) => (
              <GuideRow
                channel={r.c}
                top={headPx() + r.i() * rowPx()}
                height={rowPx()}
                chW={chPx()}
                cells={rowModel(r.c.id)}
                range={drawRange()}
                xOf={xOf}
                textX={view().left + chPx()}
                now={t()}
                focusedKey={focused()?.key}
                onWatch={watch}
                onOpen={setDetail}
                onFocusCell={(c) => {
                  setFocused(c);
                  setBrowsingChannel(c.ch);
                  if (!(c.start <= cursorT && c.stop > cursorT)) cursorT = Math.max(c.start, timeRange()[0]);
                }}
              />
            )}
          </For>

          <Show when={t() >= origin && t() < origin + span()}>
            <div class="guide__now" style={{ left: `${nowX()}px`, top: `${headPx()}px`, height: `${canvasH() - headPx()}px` }} aria-hidden="true" />
          </Show>
        </div>
      </div>

      <Show when={!filteredList()}>
        <div class="guide__empty guide__empty--loading" role="status"><span class="spinner" /> <span>Loading channels…</span></div>
      </Show>
      <Show when={filteredList() && emptyView()}>
        <div class="guide__empty" role="status">
          <Icon name="clock" class="guide__empty-icon" />
          <h2>{providerHasNoGuide() ? "No guide data for this provider yet" : "No guide data for these channels"}</h2>
          <p>
            {providerHasNoGuide()
              ? "Your provider hasn't sent a TV guide yet, so there's nothing to show here. Reel loads it together with the channel list — the grid fills in after the next refresh. Live TV works in the meantime."
              : "These channels don't have programme information from your provider. Try another group, or just watch."}
          </p>
          <div class="actions actions--center">
            <a class="btn btn--primary btn--sm" href={href("/live")}><Icon name="live" /> <span>Watch Live TV</span></a>
            <Show when={isAdmin()}>
              <a class="btn btn--sm" href={href("/admin/iptv")}><Icon name="gear" /> <span>Live TV settings</span></a>
            </Show>
          </div>
        </div>
      </Show>

      <Show when={digits()}>
        <div class="numentry numentry--guide" role="status">
          <div class="numentry__digits"><For each={digits().split("")}>{(d) => <span>{d}</span>}</For></div>
          <span class="numentry__name">{channelByNumber(Number(digits()))?.name ?? "\u00a0"}</span>
        </div>
      </Show>

      <Show when={detail()}>
        {(c) => (
          <Details
            cell={c()}
            onClose={() => {
              const cell = c();
              setDetail(undefined);
              requestAnimationFrame(() => focusCell(cell));
            }}
            onWatch={() => watch(c().ch)}
          />
        )}
      </Show>
    </div>
  );
}

function GuideRow(props: {
  channel: IptvChannel;
  top: number;
  height: number;
  chW: number;
  cells: Cell[];
  range: readonly [number, number];
  xOf: (t: number) => number;
  /** Left edge of the visible programme area: titles of cells that started earlier slide to it. */
  textX: number;
  now: number;
  focusedKey?: string;
  onWatch: (ch: number) => void;
  onOpen: (c: Cell) => void;
  onFocusCell: (c: Cell) => void;
}): JSX.Element {
  const visible = createMemo(() => {
    const [a, b] = props.range;
    return props.cells.filter((c) => c.stop > a && c.start < b);
  });
  const c = () => props.channel;
  return (
    <div class="grow" style={{ transform: `translateY(${props.top}px)`, height: `${props.height}px` }} role="row">
      <button
        type="button"
        class="grow__ch"
        style={{ width: `${props.chW}px` }}
        data-fid={`gch-${c().id}`}
        data-guide-ch={c().id}
        aria-label={`Watch ${c().name}`}
        onClick={() => props.onWatch(c().id)}
      >
        <span class="grow__num">{c().number ?? ""}</span>
        <ChannelLogo channel={c()} />
        <span class="grow__name">{c().name}</span>
      </button>
      <For each={visible()}>
        {(cell) => {
          const x0 = () => Math.max(props.chW, props.xOf(cell.start));
          const w = () => props.xOf(cell.stop) - x0();
          const airing = () => cell.start <= props.now && cell.stop > props.now;
          // Keep the title readable when the programme started off screen (manual sticky: no
          // position:sticky inside <button> on older Chromium).
          const shift = () => Math.max(0, Math.min(props.textX - x0(), w() - 120));
          const past = () => cell.stop <= props.now;
          return (
            <button
              type="button"
              class={`gcell gcell--${cell.kind}`}
              classList={{ "is-airing": airing(), "is-past": past(), "is-focused": props.focusedKey === cell.key }}
              style={{ left: `${x0()}px`, width: `${Math.max(2, w() - 3)}px` }}
              tabindex={cell.kind === "loading" ? -1 : 0}
              data-focusable={cell.kind === "loading" ? "" : undefined}
              data-fid={cell.key}
              aria-label={cell.prog ? `${cell.prog.title}, ${fmtRange(cell.prog)}${airing() ? ", on now" : ""}` : cell.kind === "loading" ? "Loading" : "No information"}
              onFocus={() => props.onFocusCell(cell)}
              onClick={() => (cell.kind === "prog" ? props.onOpen(cell) : airing() ? props.onWatch(cell.ch) : undefined)}
            >
              <span class="gcell__in" style={{ transform: shift() ? `translateX(${shift()}px)` : undefined, "max-width": shift() ? `calc(100% - ${shift()}px)` : undefined }}>
                <Switch>
                  <Match when={cell.kind === "prog"}>
                    <span class="gcell__title">{cell.prog!.title}</span>
                    <span class="gcell__time">
                      {fmtTime(cell.start)}
                      <Show when={cell.prog!.sub_title}> · {cell.prog!.sub_title}</Show>
                    </span>
                  </Match>
                  <Match when={cell.kind === "gap"}><span class="gcell__time">No information</span></Match>
                  <Match when={cell.kind === "loading"}><span class="gcell__skel" /></Match>
                </Switch>
              </span>
              <Show when={airing() && cell.kind === "prog"}>
                <span class="gcell__bar" aria-hidden="true"><i style={{ width: `${progressOf(cell.prog, props.now) * 100}%` }} /></span>
              </Show>
            </button>
          );
        }}
      </For>
    </div>
  );
}

/** The strip above the grid describing the focused programme. */
function Preview(props: { cell?: Cell }): JSX.Element {
  const ch = () => (props.cell ? channelById(props.cell.ch) : undefined);
  return (
    <div class="guide__preview" aria-live="polite">
      <Show when={props.cell && ch()} fallback={<p class="guide__preview-hint">Arrows to browse · OK for details · 0–9 jump to a channel</p>}>
        <ChannelLogo channel={ch()!} class="chlogo--lg" />
        <div class="guide__preview-body">
          <p class="guide__preview-meta">
            <span>{ch()!.number != null ? `${ch()!.number} · ` : ""}{ch()!.name}</span>
            <span> · {fmtDay(props.cell!.start)} {fmtTime(props.cell!.start)} – {fmtTime(props.cell!.stop)}</span>
            <Show when={props.cell!.prog?.category}><span> · {props.cell!.prog!.category}</span></Show>
          </p>
          <h2 class="guide__preview-title">
            {props.cell!.prog?.title ?? (props.cell!.kind === "loading" ? "Loading…" : "No information")}
            <Show when={props.cell!.prog?.sub_title}><span class="guide__preview-sub"> {props.cell!.prog!.sub_title}</span></Show>
          </h2>
        </div>
        <Show when={props.cell!.start <= clock() && props.cell!.stop > clock()}>
          <span class="guide__onnow"><i /> On now</span>
        </Show>
      </Show>
    </div>
  );
}

function Details(props: { cell: Cell; onClose: () => void; onWatch: () => void }): JSX.Element {
  let box!: HTMLDivElement;
  const ch = () => channelById(props.cell.ch);
  const [full] = createResource(
    () => props.cell,
    async (c) => {
      const g = await iptv.epg({ from: c.start, to: Math.max(c.stop, c.start + 60), channels: [c.ch], detail: true });
      return g.channels[0]?.programs.find((p) => p.start === c.start) ?? c.prog;
    },
  );
  const p = () => full() ?? props.cell.prog;
  const airing = () => props.cell.start <= clock() && props.cell.stop > clock();
  onMount(() => {
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(off);
    requestAnimationFrame(() => box.querySelector<HTMLElement>("[data-autofocus]")?.focus({ preventScroll: true }));
  });
  return (
    <>
      <div class="gdetail__scrim" onClick={props.onClose} aria-hidden="true" />
      <div ref={box} class="gdetail" role="dialog" aria-modal="true" aria-label={p()?.title ?? "Programme"} data-nav-trap>
        <div class="gdetail__head">
          <Show when={ch()}>{(c) => <ChannelLogo channel={c()} class="chlogo--lg" />}</Show>
          <div>
            <p class="gdetail__meta">
              {ch()?.number != null ? `${ch()!.number} · ` : ""}{ch()?.name}
            </p>
            <p class="gdetail__when">
              {fmtDay(props.cell.start)} · {fmtTime(props.cell.start)} – {fmtTime(props.cell.stop)} · {Math.round((props.cell.stop - props.cell.start) / 60)} min
            </p>
          </div>
          <Show when={airing()}><span class="guide__onnow"><i /> On now</span></Show>
        </div>
        <h2 class="gdetail__title">{p()?.title}</h2>
        <Show when={p()?.sub_title || p()?.episode_num}>
          <p class="gdetail__sub">{[p()?.sub_title, p()?.episode_num].filter(Boolean).join(" · ")}</p>
        </Show>
        <Show when={airing()}>
          <div class="nowp__timeline gdetail__timeline">
            <span class="nowp__bar"><i style={{ width: `${progressOf(props.cell, clock()) * 100}%` }} /></span>
            <span class="nowp__left">{Math.max(0, Math.round((props.cell.stop - clock()) / 60))} min left</span>
          </div>
        </Show>
        <p class="gdetail__desc" classList={{ "is-loading": full.loading }}>
          {full.loading ? "\u00a0" : p()?.description || "No description for this programme."}
        </p>
        <Show when={p()?.category}><span class="pill pill--status">{p()!.category}</span></Show>
        <div class="actions gdetail__actions">
          <Show
            when={airing()}
            fallback={
              <button type="button" class="btn" data-autofocus onClick={props.onWatch}>
                <Icon name="live" /> <span>Watch {ch()?.name ?? "channel"} now</span>
              </button>
            }
          >
            <button type="button" class="btn btn--primary" data-autofocus onClick={props.onWatch}><Icon name="play" /> <span>Watch</span></button>
          </Show>
          <button type="button" class="btn btn--quiet" onClick={props.onClose}><Icon name="x" /> <span>Close</span></button>
        </div>
      </div>
    </>
  );
}

function cssEsc(s: string): string {
  return s.replace(/["\\]/g, "\\$&");
}


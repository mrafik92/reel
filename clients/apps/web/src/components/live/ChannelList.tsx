import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { focusElement, Icon } from "@reel/ui";
import type { IptvChannel } from "../../api/iptv";
import { ChannelLogo } from "./ChannelLogo";
import { clock, ensureNow, fmtTime, nowNext, progressOf } from "./store";

export interface ChannelListApi {
  /** Scroll a channel into view (centred) and optionally focus it. */
  reveal(id: number, focus?: boolean): void;
}

export interface ChannelListProps {
  channels: IptvChannel[];
  /** Tuned channel (gets the live marker). */
  activeId?: number;
  /** Channel under a pending zap / number entry (highlighted while it tunes). */
  pendingId?: number;
  onPick: (c: IptvChannel) => void;
  onFocusChannel?: (c: IptvChannel) => void;
  controller?: (api: ChannelListApi) => void;
  /** Row height in rem. */
  rowRem?: number;
  class?: string;
  label?: string;
  empty?: JSX.Element;
  /** Changing this scrolls back to the top (e.g. the active filter). */
  resetKey?: string;
}

const OVERSCAN = 6;

function rootPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

/**
 * Virtualised channel list in its own scroller. Only the rows near the viewport exist in the DOM, so
 * thousands of channels stay smooth on TV browsers; D-pad focus scrolls via `data-nav-scroll="y"`.
 */
export function ChannelList(props: ChannelListProps): JSX.Element {
  let el!: HTMLDivElement;
  const [rem, setRem] = createSignal(16);
  const [view, setView] = createSignal({ top: 0, height: 800 });
  const rowPx = () => (props.rowRem ?? 5) * rem();

  const range = createMemo(() => {
    const n = props.channels.length;
    const h = rowPx();
    const v = view();
    const start = Math.max(0, Math.floor(v.top / h) - OVERSCAN);
    const end = Math.min(n, Math.ceil((v.top + v.height) / h) + OVERSCAN);
    return [start, end] as const;
  });

  const visible = createMemo(() => {
    const [s, e] = range();
    const out: { c: IptvChannel; i: number }[] = [];
    for (let i = s; i < e; i++) out.push({ c: props.channels[i]!, i });
    return out;
  });

  // Stable row objects per channel id so <For> keeps DOM nodes (and focus) across scrolls.
  const rowCache = new Map<number, { c: IptvChannel; i: () => number; setI: (n: number) => void; setC: (c: IptvChannel) => void; ch: () => IptvChannel }>();
  const rows = createMemo(() => {
    const out = visible().map(({ c, i }) => {
      let r = rowCache.get(c.id);
      if (!r) {
        const [gi, si] = createSignal(i);
        const [gc, sc] = createSignal(c);
        r = { c, i: gi, setI: si, ch: gc, setC: sc };
        rowCache.set(c.id, r);
      } else {
        r.setI(i);
        r.setC(c);
      }
      return r;
    });
    if (rowCache.size > 400) {
      const keep = new Set(out.map((r) => r.c.id));
      rowCache.forEach((_v, k) => !keep.has(k) && rowCache.delete(k));
    }
    return out;
  });

  // Now/next for whatever is on screen.
  createEffect(() => {
    const [s, e] = range();
    const ids: number[] = [];
    for (let i = s; i < e; i++) ids.push(props.channels[i]!.id);
    if (ids.length) ensureNow(ids);
  });

  const measure = () => {
    if (!el) return;
    setView({ top: el.scrollTop, height: el.clientHeight || 800 });
  };
  let raf = 0;
  const onScroll = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      measure();
    });
  };

  onMount(() => {
    setRem(rootPx());
    measure();
    const onResize = () => {
      setRem(rootPx());
      measure();
    };
    window.addEventListener("resize", onResize);
    onCleanup(() => {
      window.removeEventListener("resize", onResize);
      if (raf) cancelAnimationFrame(raf);
    });
  });

  // Filter changed → back to the top (not on every list update, e.g. a favourite toggled).
  createEffect(on(() => props.resetKey, () => {
    if (el) el.scrollTop = 0;
    measure();
  }, { defer: true }));

  const reveal = (id: number, focus = false) => {
    const i = props.channels.findIndex((c) => c.id === id);
    if (i < 0 || !el) return;
    const h = rowPx();
    const target = i * h - (el.clientHeight - h) / 2;
    el.scrollTop = Math.max(0, target);
    measure();
    if (focus) {
      // Wait for the row to render at its new position.
      requestAnimationFrame(() => {
        const row = el.querySelector<HTMLElement>(`[data-fid="ch-${id}"]`);
        if (row) focusElement(row, { smooth: false });
      });
    }
  };
  props.controller?.({ reveal });

  return (
    <div
      ref={el}
      class={"chlist" + (props.class ? " " + props.class : "")}
      data-nav-scroll="y"
      data-nav-group
      role="listbox"
      aria-label={props.label ?? "Channels"}
      onScroll={onScroll}
    >
      <Show when={props.channels.length} fallback={props.empty}>
        <div class="chlist__track" style={{ height: `${props.channels.length * (props.rowRem ?? 5)}rem` }}>
          <For each={rows()}>
            {(r) => {
              const c = r.ch;
              const nn = () => nowNext(c().id);
              const live = () => props.activeId === c().id;
              return (
                <button
                  type="button"
                  class="chrow"
                  classList={{ "is-live": live(), "is-pending": props.pendingId === c().id }}
                  style={{ transform: `translateY(${r.i() * (props.rowRem ?? 5)}rem)`, height: `${props.rowRem ?? 5}rem` }}
                  data-fid={`ch-${c().id}`}
                  data-nav-default={live() ? "" : undefined}
                  role="option"
                  aria-selected={live()}
                  aria-label={`${c().number != null ? c().number + " " : ""}${c().name}${nn()?.now ? `, now: ${nn()!.now!.title}` : ""}`}
                  onClick={() => props.onPick(c())}
                  onFocus={() => props.onFocusChannel?.(c())}
                >
                  <span class="chrow__num">{c().number ?? "–"}</span>
                  <ChannelLogo channel={c()} />
                  <span class="chrow__body">
                    <span class="chrow__name">
                      {c().name}
                      <Show when={c().favorite}><Icon name="star" class="chrow__fav" /></Show>
                    </span>
                    <Show
                      when={nn()?.now}
                      fallback={<span class="chrow__now chrow__now--none">{nn() ? (c().has_epg ? "No information" : "No guide") : "\u00a0"}</span>}
                    >
                      {(p) => (
                        <>
                          <span class="chrow__now">
                            <span class="chrow__time">{fmtTime(p().start)}</span> {p().title}
                          </span>
                          <span class="chrow__bar" aria-hidden="true"><i style={{ width: `${progressOf(p(), clock()) * 100}%` }} /></span>
                        </>
                      )}
                    </Show>
                  </span>
                  <Show when={live()}>
                    <span class="chrow__eq" aria-hidden="true"><i /><i /><i /></span>
                  </Show>
                </button>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}

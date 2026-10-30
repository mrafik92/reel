import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { focusElement, Icon } from "@reel/ui";
import type { IptvChannel } from "../../api/iptv";
import { ChannelLogo } from "./ChannelLogo";
import type { ChannelListApi } from "./ChannelList";
import { clock, ensureNow, fmtTime, nowNext, progressOf } from "./store";

const CARD_REM = 14.5;
const GAP_REM = 1;
const OVERSCAN = 4;

function rootPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

/**
 * The channels of the selected group as one horizontal row of cards, virtualised on the x axis
 * (a group can hold thousands). D-pad Left/Right walks it (`data-nav-scroll="x"` keeps focus in view).
 */
export function ChannelRail(props: {
  channels: IptvChannel[];
  activeId?: number;
  pendingId?: number;
  onPick: (c: IptvChannel) => void;
  controller?: (api: ChannelListApi) => void;
  resetKey?: string;
  empty?: JSX.Element;
}): JSX.Element {
  let el!: HTMLDivElement;
  const [rem, setRem] = createSignal(16);
  const [view, setView] = createSignal({ left: 0, width: 1400 });
  const step = () => (CARD_REM + GAP_REM) * rem();

  const range = createMemo(() => {
    const n = props.channels.length;
    const v = view();
    const s = Math.max(0, Math.floor(v.left / step()) - OVERSCAN);
    const e = Math.min(n, Math.ceil((v.left + v.width) / step()) + OVERSCAN);
    return [s, e] as const;
  });

  // Stable card objects per channel so <For> keeps DOM nodes (and focus) while scrolling.
  type Ref = { id: number; c: () => IptvChannel; setC: (c: IptvChannel) => void; i: () => number; setI: (n: number) => void };
  const refs = new Map<number, Ref>();
  const cards = createMemo(() => {
    const [s, e] = range();
    const out: Ref[] = [];
    for (let i = s; i < e; i++) {
      const c = props.channels[i]!;
      let r = refs.get(c.id);
      if (!r) {
        const [gc, sc] = createSignal(c);
        const [gi, si] = createSignal(i);
        r = { id: c.id, c: gc, setC: sc, i: gi, setI: si };
        refs.set(c.id, r);
      } else {
        r.setC(c);
        r.setI(i);
      }
      out.push(r);
    }
    if (refs.size > 300) {
      const keep = new Set(out.map((r) => r.id));
      refs.forEach((_v, k) => !keep.has(k) && refs.delete(k));
    }
    return out;
  });

  createEffect(() => {
    const [s, e] = range();
    const ids: number[] = [];
    for (let i = s; i < e; i++) ids.push(props.channels[i]!.id);
    if (ids.length) ensureNow(ids);
  });

  const measure = () => el && setView({ left: el.scrollLeft, width: el.clientWidth || 1400 });
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
  createEffect(on(() => props.resetKey, () => {
    if (el) el.scrollLeft = 0;
    measure();
  }, { defer: true }));

  const reveal = (id: number, focus = false) => {
    const i = props.channels.findIndex((c) => c.id === id);
    if (i < 0 || !el) return;
    // Keep the card a little in from the left edge, like the other rows.
    el.scrollLeft = Math.max(0, i * step() - step() * 1.5);
    measure();
    if (focus)
      requestAnimationFrame(() => {
        const card = el.querySelector<HTMLElement>(`[data-fid="ch-${id}"]`);
        if (card) focusElement(card, { smooth: false });
      });
  };
  props.controller?.({ reveal });

  return (
    <div ref={el} class="chrail" data-nav-scroll="x" data-nav-group role="listbox" aria-label="Channels" aria-orientation="horizontal" onScroll={onScroll}>
      <Show when={props.channels.length} fallback={<div class="chrail__empty">{props.empty}</div>}>
        <div class="chrail__track" style={{ width: `${props.channels.length * (CARD_REM + GAP_REM)}rem` }}>
          <For each={cards()}>
            {(r) => {
              const c = r.c;
              const nn = () => nowNext(c().id);
              const live = () => props.activeId === c().id;
              return (
                <button
                  type="button"
                  class="chcard"
                  classList={{ "is-live": live(), "is-pending": props.pendingId === c().id }}
                  style={{ left: `${r.i() * (CARD_REM + GAP_REM)}rem`, width: `${CARD_REM}rem` }}
                  data-fid={`ch-${c().id}`}
                  data-nav-default={live() ? "" : undefined}
                  role="option"
                  aria-selected={live()}
                  aria-label={`${c().number != null ? c().number + " " : ""}${c().name}${nn()?.now ? `, now: ${nn()!.now!.title}` : ""}`}
                  onClick={() => props.onPick(c())}
                >
                  <span class="chcard__art">
                    <ChannelLogo channel={c()} class="chcard__logo" />
                    <span class="chcard__num">{c().number ?? ""}</span>
                    <Show when={c().favorite}><Icon name="star" class="chcard__fav" /></Show>
                    <Show when={live()}>
                      <span class="chcard__live" aria-hidden="true"><span class="chrow__eq"><i /><i /><i /></span> Live</span>
                    </Show>
                  </span>
                  <span class="chcard__body">
                    <span class="chcard__name" dir="auto">{c().name}</span>
                    <Show
                      when={nn()?.now}
                      fallback={<span class="chcard__now chcard__now--none">{nn() ? (c().has_epg ? "No information" : "No guide") : "\u00a0"}</span>}
                    >
                      {(p) => (
                        <>
                          <span class="chcard__now"><span class="chcard__time">{fmtTime(p().start)}</span> {p().title}</span>
                          <span class="chcard__bar" aria-hidden="true"><i style={{ width: `${progressOf(p(), clock()) * 100}%` }} /></span>
                        </>
                      )}
                    </Show>
                  </span>
                </button>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}

import { createEffect, createMemo, createSignal, For, onCleanup, onMount, type JSX } from "solid-js";

export interface VirtualGridProps<T> {
  /** Loaded items (stable object references keep DOM nodes alive). */
  items: T[];
  /** Total slots (e.g. library item_count); unloaded slots render placeholders. */
  total: number;
  /** Minimum column width in rem. */
  minColumnRem: number;
  /** Art height / width ratio (posters: 1.5). */
  ratio: number;
  /** Extra height under the art per cell, in rem (caption). */
  captionRem: number;
  /** Gap in rem (column and row). */
  gapRem: number;
  /** Rows rendered beyond the viewport on each side. */
  overscanRows?: number;
  key?: (item: T) => string | number;
  renderItem: (item: T, index: () => number) => JSX.Element;
  renderPlaceholder?: (index: number) => JSX.Element;
  /** Called with the last index needed by the viewport (+overscan). */
  onRange?: (start: number, end: number) => void;
  class?: string;
}

function rootPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

/**
 * Window-scrolled virtualized grid. Only rows near the viewport are in the DOM,
 * which keeps old TV browsers responsive with thousands of posters.
 */
export function VirtualGrid<T>(props: VirtualGridProps<T>): JSX.Element {
  let el!: HTMLDivElement;
  const [width, setWidth] = createSignal(0);
  const [rem, setRem] = createSignal(16);
  const [range, setRange] = createSignal<[number, number]>([0, 0]);

  const layout = createMemo(() => {
    const r = rem();
    const gap = props.gapRem * r;
    const w = width();
    const cols = Math.max(2, Math.floor((w + gap) / (props.minColumnRem * r + gap)));
    const colW = w > 0 ? (w - gap * (cols - 1)) / cols : 0;
    const cellH = colW * props.ratio + props.captionRem * r;
    return { cols, colW, cellH, rowH: cellH + gap, gap };
  });
  const rows = () => Math.ceil(props.total / layout().cols);

  let raf = 0;
  const measure = () => {
    raf = 0;
    if (!el) return;
    const L = layout();
    if (!L.rowH) return;
    const top = el.getBoundingClientRect().top;
    const vh = window.innerHeight;
    const over = props.overscanRows ?? 2;
    const first = Math.max(0, Math.floor(-top / L.rowH) - over);
    const last = Math.min(rows(), Math.ceil((vh - top) / L.rowH) + over);
    const next: [number, number] = [first * L.cols, Math.min(props.total, last * L.cols)];
    const cur = range();
    if (cur[0] !== next[0] || cur[1] !== next[1]) {
      setRange(next);
      props.onRange?.(next[0], next[1]);
    }
  };
  const schedule = () => {
    if (!raf) raf = requestAnimationFrame(measure);
  };
  const resize = () => {
    setRem(rootPx());
    setWidth(el.clientWidth);
    schedule();
  };

  onMount(() => {
    resize();
    window.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", resize);
    let ro: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(resize);
      ro.observe(el);
    }
    onCleanup(() => {
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", resize);
      ro?.disconnect();
      if (raf) cancelAnimationFrame(raf);
    });
  });

  // Re-measure when data/size changes.
  createEffect(() => {
    props.total;
    props.items.length;
    layout();
    schedule();
  });

  const visible = createMemo(() => {
    const [a, b] = range();
    return props.items.slice(a, Math.min(b, props.items.length));
  });
  const placeholders = createMemo(() => {
    const [a, b] = range();
    const from = Math.max(a, props.items.length);
    const out: number[] = [];
    for (let i = from; i < b; i++) out.push(i);
    return out;
  });
  // Index lookup for absolute placement without re-keying.
  const indexOf = createMemo(() => {
    const m = new Map<T, number>();
    const [a] = range();
    visible().forEach((it, i) => m.set(it, a + i));
    return m;
  });

  const pos = (i: number): JSX.CSSProperties => {
    const L = layout();
    const c = i % L.cols;
    const r = Math.floor(i / L.cols);
    return {
      width: `${L.colW}px`,
      height: `${L.cellH}px`,
      transform: `translate(${c * (L.colW + L.gap)}px, ${r * L.rowH}px)`,
    };
  };

  return (
    <div
      ref={el}
      class={"vgrid" + (props.class ? " " + props.class : "")}
      style={{ height: `${Math.max(0, rows() * layout().rowH - layout().gap)}px` }}
      data-nav-group="free"
      role="list"
    >
      <For each={visible()}>
        {(item) => {
          const idx = () => indexOf().get(item) ?? 0;
          return (
            <div class="vgrid__cell" style={pos(idx())} role="listitem">
              {props.renderItem(item, idx)}
            </div>
          );
        }}
      </For>
      <For each={placeholders()}>
        {(i) => (
          <div class="vgrid__cell" style={pos(i)} aria-hidden="true">
            {props.renderPlaceholder?.(i)}
          </div>
        )}
      </For>
    </div>
  );
}

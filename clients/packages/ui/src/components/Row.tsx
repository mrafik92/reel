import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { currentTranslateX, isTvMode } from "../navigation";
import { Icon } from "./Icon";

export interface RowProps {
  title: string;
  /** Quiet line after the title, e.g. "Drama · Mystery". */
  subtitle?: string | null;
  /** Optional link right after the title ("Explore all"). */
  action?: { label: string; href: string };
  /** `ranked`: poster tiles with a large rank numeral (pass `rank` to each `PosterCard`). */
  variant?: "poster" | "landscape" | "cast" | "ranked";
  children: JSX.Element;
  class?: string;
}

/** A titled horizontal rail. Remembers focus per row; anchors vertical scroll on TV. */
export function Row(props: RowProps): JSX.Element {
  let scroller!: HTMLDivElement;
  let track!: HTMLDivElement;
  const [canPrev, setCanPrev] = createSignal(false);
  const [canNext, setCanNext] = createSignal(false);
  // The paging arrows are pointer-only: never rendered on TV, hidden by CSS on touch.
  const arrows = !isTvMode();

  const measure = () => {
    // Tiles sliding aside for the preview card (#301) widen the scrollable overflow while they move:
    // take the last tile's slide back off, so the "next" arrow doesn't light up for it.
    const last = track.lastElementChild as HTMLElement | null;
    const slide = last ? Math.max(0, currentTranslateX(last)) : 0;
    const max = scroller.scrollWidth - slide - scroller.clientWidth;
    setCanPrev(scroller.scrollLeft > 1);
    setCanNext(scroller.scrollLeft < max - 1);
  };
  const page = (dir: 1 | -1) => {
    const left = dir * scroller.clientWidth * 0.9;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (typeof scroller.scrollBy === "function") scroller.scrollBy({ left, behavior: reduce ? "auto" : "smooth" });
    else scroller.scrollLeft += left;
  };

  onMount(() => {
    if (!arrows) return;
    measure();
    scroller.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", measure);
    // Tiles arrive after the row mounts (lazy lanes); ResizeObserver isn't available on the TV floor.
    const mo = new MutationObserver(measure);
    mo.observe(track, { childList: true });
    onCleanup(() => {
      scroller.removeEventListener("scroll", measure);
      window.removeEventListener("resize", measure);
      mo.disconnect();
    });
  });

  const Arrow = (p: { dir: 1 | -1 }) => (
    <button
      type="button"
      class={`row__nav row__nav--${p.dir < 0 ? "prev" : "next"}`}
      classList={{ "is-on": p.dir < 0 ? canPrev() : canNext() }}
      tabindex="-1"
      data-nav-skip
      aria-hidden="true"
      onClick={() => page(p.dir)}
    >
      <Icon name={p.dir < 0 ? "back" : "forward"} />
    </button>
  );

  return (
    <section
      class={`row row--${props.variant ?? "poster"}${props.class ? " " + props.class : ""}`}
      classList={{ "has-prev": canPrev(), "has-next": canNext() }}
      data-nav-anchor
      data-nav-group
    >
      <header class="row__head">
        <h2 class="row__title">
          {props.title}
          <Show when={props.subtitle}>
            <span class="row__sub">{props.subtitle}</span>
          </Show>
        </h2>
        <Show when={props.action}>
          {(a) => (
            <a class="row__action" href={a().href}>
              <span>{a().label}</span>
              <Icon name="forward" />
            </a>
          )}
        </Show>
      </header>
      <div class="row__body">
        <Show when={arrows}>
          <Arrow dir={-1} />
        </Show>
        <div ref={scroller} class="row__scroller" data-nav-scroll="x">
          <div ref={track} class="row__track">{props.children}</div>
        </div>
        <Show when={arrows}>
          <Arrow dir={1} />
        </Show>
      </div>
    </section>
  );
}

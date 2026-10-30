import { Show, type JSX } from "solid-js";

export interface RowProps {
  title: string;
  /** Quiet line after the title, e.g. "Drama · Mystery". */
  subtitle?: string | null;
  /** Small label before the title, e.g. "01". */
  index?: string;
  /** Optional trailing link (e.g. "See all"). */
  action?: { label: string; href: string };
  variant?: "poster" | "landscape" | "cast";
  children: JSX.Element;
  class?: string;
}

/** A titled horizontal rail. Remembers focus per row; anchors vertical scroll on TV. */
export function Row(props: RowProps): JSX.Element {
  return (
    <section
      class={`row row--${props.variant ?? "poster"}${props.class ? " " + props.class : ""}`}
      data-nav-anchor
      data-nav-group
    >
      <header class="row__head">
        <h2 class="row__title">
          <Show when={props.index}>
            <span class="row__index">{props.index}</span>
          </Show>
          {props.title}
          <Show when={props.subtitle}>
            <span class="row__sub">{props.subtitle}</span>
          </Show>
        </h2>
        <Show when={props.action}>
          {(a) => (
            <a class="row__action" href={a().href}>
              {a().label}
            </a>
          )}
        </Show>
      </header>
      <div class="row__scroller" data-nav-scroll="x">
        <div class="row__track">{props.children}</div>
      </div>
    </section>
  );
}

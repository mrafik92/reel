import { createSignal, For, Show, type JSX } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { Backdrop, Icon, Sheet } from "@reel/ui";
import { AMBIENT_W, useAmbientArt } from "../lib/ambient";
import { TV } from "../lib/mode";

/**
 * The title page's frame: the title's art, stretched and dimmed, fills the page behind one glass
 * sheet. `head` sits over the lower part of the sheet's backdrop. Still a route (#173 is the true
 * overlay), so ✕ just goes back. TV has no ✕: Back on the remote does the same.
 */
export function TitleSheet(props: { backdrop?: string | null; head: JSX.Element; children?: JSX.Element }): JSX.Element {
  const navigate = useNavigate();
  const ambient = useAmbientArt(() => props.backdrop);
  const close = () => {
    if (window.history.length > 1) window.history.back();
    else navigate("/");
  };
  return (
    <>
      <div class="title-ambient" aria-hidden="true">
        <Show when={props.backdrop}>
          <Backdrop src={ambient()} width={AMBIENT_W} class="title-ambient__art" />
        </Show>
        <div class="title-ambient__fade" />
      </div>
      <article class="tsheet glass">
        <Show when={!TV}>
          <button type="button" class="iconbtn tsheet__close glass" aria-label="Close" title="Close" onClick={close}>
            <Icon name="x" />
          </button>
        </Show>
        <div class="tsheet__top" data-nav-anchor style={{ "--nav-anchor-top": "0px" }}>
          <Backdrop src={props.backdrop} class="tsheet__art" />
          <div class="tsheet__head">{props.head}</div>
        </div>
        {props.children}
      </article>
    </>
  );
}

/**
 * A round action with a tooltip; on phones the label shows under it. Links when `href` is set.
 */
export function RoundAction(props: {
  icon: JSX.Element;
  label: string;
  href?: string;
  pressed?: boolean;
  disabled?: boolean;
  onClick?: () => void;
}): JSX.Element {
  const inner = () => (
    <>
      <span class="iconbtn iconbtn--ring" aria-hidden="true">{props.icon}</span>
      <span class="tact__label">{props.label}</span>
    </>
  );
  return (
    <Show
      when={props.href}
      fallback={
        <button
          type="button"
          class="tact"
          title={props.label}
          aria-label={props.label}
          aria-pressed={props.pressed === undefined ? undefined : props.pressed}
          aria-disabled={props.disabled ? "true" : undefined}
          onClick={() => props.onClick?.()}
        >
          {inner()}
        </button>
      }
    >
      <a class="tact" href={props.href} title={props.label} aria-label={props.label}>
        {inner()}
      </a>
    </Show>
  );
}

/** The season dropdown of the episode list. A single season is plain text. */
export function SeasonPicker<V>(props: {
  seasons: { value: V; label: string }[];
  selected: V;
  onSelect: (v: V) => void;
}): JSX.Element {
  const [open, setOpen] = createSignal(false);
  const current = () => props.seasons.find((s) => s.value === props.selected) ?? props.seasons[0];
  return (
    <Show when={props.seasons.length > 1} fallback={<span class="season-single" dir="auto">{current()?.label}</span>}>
      <button type="button" class="btn btn--dropdown" aria-haspopup="dialog" aria-expanded={open() ? "true" : "false"} onClick={() => setOpen(true)}>
        <span dir="auto">{current()?.label}</span>
        <Icon name="down" />
      </button>
      <Show when={open()}>
        <Sheet
          title="Seasons"
          selected={props.selected}
          options={props.seasons.map((s) => ({ value: s.value, title: s.label }))}
          onSelect={(v) => props.onSelect(v)}
          onClose={() => setOpen(false)}
        />
      </Show>
    </Show>
  );
}

/** "Label: a, b, c" in the title page's side column. */
export function InfoLine(props: { label: string; children: JSX.Element }): JSX.Element {
  return (
    <p class="tinfo__line">
      <span class="tinfo__key">{props.label}: </span>
      {props.children}
    </p>
  );
}

/** The full cast as text, for the About section. */
export function CastText(props: { names: string[] }): JSX.Element {
  return (
    <For each={props.names}>
      {(n, i) => (
        <>
          {i() > 0 ? ", " : ""}
          {n}
        </>
      )}
    </For>
  );
}

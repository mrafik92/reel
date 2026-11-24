import { createEffect, createSignal, on, onCleanup, Show, type JSX } from "solid-js";
import { useLocation } from "@solidjs/router";
import { focusFirst, Icon, isTvMode, onBack } from "@reel/ui";
import { MOCK, href } from "../lib/mode";

const HOVER_OPEN_MS = 150;
const HOVER_CLOSE_MS = 220;

export interface TopMenuProps {
  /** Accessible name of the trigger. */
  label: string;
  buttonClass: string;
  button: JSX.Element;
  /** Also opens when the pointer rests on the trigger. */
  hover?: boolean;
  align?: "left" | "right";
  class?: string;
  children: (close: () => void) => JSX.Element;
}

/**
 * A dropdown menu in the top bar. While open it is a `data-nav-trap`, so the arrow keys and the
 * D-pad move between its items; Escape or Back closes it and focus returns to the trigger.
 */
export function TopMenu(props: TopMenuProps): JSX.Element {
  const loc = useLocation();
  const [open, setOpen] = createSignal(false);
  let root!: HTMLDivElement;
  let trigger!: HTMLButtonElement;
  let panel: HTMLDivElement | undefined;
  let openedByHover = false;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;

  const clearHover = () => {
    if (hoverTimer) clearTimeout(hoverTimer);
    hoverTimer = undefined;
  };
  const close = (refocus = true) => {
    clearHover();
    if (!open()) return;
    const inside = !!panel && panel.contains(document.activeElement);
    setOpen(false);
    if (refocus && inside) trigger.focus({ preventScroll: true });
  };
  const show = (byHover: boolean) => {
    openedByHover = byHover;
    setOpen(true);
    // A keyboard or remote lands on the first item; a mouse leaves focus where it is.
    if (!byHover && document.documentElement.getAttribute("data-input") === "key" && panel) focusFirst(panel);
  };

  // `on` so the effect tracks the route only: `close` reads `open()`.
  createEffect(on(() => loc.pathname, () => close(false), { defer: true }));
  createEffect(() => {
    if (!open()) return;
    const offBack = onBack(() => {
      close();
      return true;
    });
    const onDown = (e: Event) => {
      if (!root.contains(e.target as Node)) close(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    onCleanup(() => {
      offBack();
      document.removeEventListener("pointerdown", onDown, true);
    });
  });
  onCleanup(clearHover);

  const canHover = () =>
    !!props.hover &&
    !isTvMode() &&
    document.documentElement.getAttribute("data-input") === "pointer" &&
    window.matchMedia("(hover: hover)").matches;

  return (
    <div
      ref={root}
      class={`topmenu${props.class ? " " + props.class : ""}`}
      onMouseEnter={() => {
        if (!canHover()) return;
        clearHover();
        if (!open()) hoverTimer = setTimeout(() => show(true), HOVER_OPEN_MS);
      }}
      onMouseLeave={() => {
        clearHover();
        if (open() && openedByHover) hoverTimer = setTimeout(() => close(false), HOVER_CLOSE_MS);
      }}
      onFocusOut={(e) => {
        const next = e.relatedTarget as Node | null;
        if (next && !root.contains(next)) close(false);
      }}
    >
      <button
        ref={trigger}
        type="button"
        class={props.buttonClass}
        aria-label={props.label}
        aria-haspopup="menu"
        aria-expanded={open() ? "true" : "false"}
        onClick={() => {
          clearHover();
          if (open() && openedByHover) openedByHover = false;
          else if (open()) close();
          else show(false);
        }}
      >
        {props.button}
      </button>
      <Show when={open()}>
        <div
          ref={panel}
          class={`topmenu__panel glass topmenu__panel--${props.align ?? "right"}`}
          role="menu"
          aria-label={props.label}
          data-nav-trap
        >
          {props.children(() => close())}
        </div>
      </Show>
    </div>
  );
}

export function AccountMenu(props: {
  username?: string;
  settingsPath: string;
  onSignOut: () => void;
}): JSX.Element {
  const initial = () => (props.username ?? "?").charAt(0).toUpperCase();
  return (
    <TopMenu
      label={`Account${props.username ? `, ${props.username}` : ""}`}
      buttonClass="topbar__account"
      hover
      button={
        <>
          <span class="topbar__avatar" aria-hidden="true">{initial()}</span>
          <span class="topbar__caret" aria-hidden="true" />
        </>
      }
    >
      {(close) => (
        <>
          <p class="topmenu__head">
            Signed in as <strong>{props.username ?? "…"}</strong>
          </p>
          <a class="topmenu__item" role="menuitem" href={href(props.settingsPath)} onClick={close}>
            <Icon name="gear" /> <span>Settings</span>
          </a>
          <Show when={MOCK || import.meta.env.DEV}>
            <a class="topmenu__item" role="menuitem" href={href("/dev/rings")} onClick={close}>
              <Icon name="layers" /> <span>Design lab</span>
            </a>
          </Show>
          <div class="topmenu__divider" role="separator" />
          <button
            type="button"
            class="topmenu__item"
            role="menuitem"
            onClick={() => {
              close();
              props.onSignOut();
            }}
          >
            <Icon name="logout" /> <span>Sign out</span>
          </button>
        </>
      )}
    </TopMenu>
  );
}

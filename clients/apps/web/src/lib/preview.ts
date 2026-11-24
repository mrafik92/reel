import { createSignal } from "solid-js";
import type { CardPreview, ItemSummary } from "@reel/ui";
import { TV } from "./mode";

/** How long a pointer or key focus rests on a tile before its preview opens. */
export const PREVIEW_DELAY_MS = 450;
/** Time to move the pointer from the tile onto the open card. */
const LEAVE_GRACE_MS = 120;

export interface PreviewTarget {
  item: ItemSummary;
  el: HTMLElement;
  href: string;
  /** `focus` is the key/remote variant: no action buttons, never takes the pointer. */
  mode: "hover" | "focus";
}

const [target, setTarget] = createSignal<PreviewTarget | null>(null);
/** The open preview, read by the single `PreviewCard` layer. */
export const previewTarget = target;

let timer: ReturnType<typeof setTimeout> | undefined;
let leaveTimer: ReturnType<typeof setTimeout> | undefined;
let pending: PreviewTarget | null = null;

const clearTimers = () => {
  if (timer) clearTimeout(timer);
  if (leaveTimer) clearTimeout(leaveTimer);
  timer = leaveTimer = undefined;
};

const wideEnough = () => window.innerWidth > 760;
const inputIs = (v: string) => document.documentElement.getAttribute("data-input") === v;
const canHover = () => !TV && wideEnough() && inputIs("pointer") && window.matchMedia("(hover: hover)").matches;
// A webOS Magic Remote reports pointer input, but on TV its pointer moves focus: that's the focus path.
const canFocus = () => wideEnough() && (TV || inputIs("key"));

function arm(next: PreviewTarget): void {
  clearTimers();
  pending = next;
  timer = setTimeout(() => {
    timer = undefined;
    pending = null;
    if (next.el.isConnected) setTarget(next);
  }, PREVIEW_DELAY_MS);
}

/** Close at once and forget anything waiting to open. */
export function closePreview(): void {
  clearTimers();
  pending = null;
  setTarget(null);
}

/**
 * The page scrolled. An open card no longer sits on its tile, so it closes. A focus preview still
 * waiting restarts its delay: moving focus scrolls the row, and that must not cancel the card.
 */
export function previewScrolled(): void {
  if (target()) setTarget(null);
  if (pending?.mode === "focus") arm(pending);
  else if (pending) closePreview();
}

/** The pointer reached the open card. */
export function keepPreview(): void {
  if (leaveTimer) clearTimeout(leaveTimer);
  leaveTimer = undefined;
}

/** Handlers for `PosterCard`'s `preview` prop, one per tile link. */
export function cardPreview(href: string): CardPreview {
  return {
    enter(item, el) {
      if (!canHover()) return;
      if (target()?.el === el) return keepPreview();
      setTarget(null);
      arm({ item, el, href, mode: "hover" });
    },
    leave(el) {
      if (pending?.el === el) closePreview();
      else if (target()?.el === el && target()!.mode === "hover") {
        if (leaveTimer) clearTimeout(leaveTimer);
        leaveTimer = setTimeout(closePreview, LEAVE_GRACE_MS);
      }
    },
    focus(item, el) {
      if (!canFocus()) return;
      // Holding an arrow: each step closes the card and restarts the delay, so none ever opens.
      setTarget(null);
      arm({ item, el, href, mode: "focus" });
    },
    blur(el) {
      const cur = target();
      if (pending?.el === el || (cur && cur.el === el && cur.mode === "focus")) closePreview();
    },
  };
}

/**
 * Spatial (D-pad) navigation for TV remotes and keyboards.
 *
 * DOM-driven and framework-agnostic: anything natively focusable (links, buttons,
 * inputs) or marked `data-focusable` takes part. Arrow keys move real DOM focus
 * to the geometrically nearest candidate, so `:focus` styles, Enter-to-click
 * and screen readers all keep working.
 *
 * Attributes:
 * - `data-nav-group`     remembers its last focused child; entering the group from
 *                        outside restores it (rows, nav rail, grids). Falls back to
 *                        `[data-nav-default]` / `[aria-current="page"]`.
 * - `data-nav-trap`      modal scope: while present, focus can't leave it.
 * - `data-nav-scroll="x"` horizontal scroller; focused children are kept in view.
 * - `data-nav-anchor`    vertical anchor (e.g. a row); on TV the page scrolls so the
 *                        anchor sits at `--nav-anchor-top` (default 30vh).
 * - `data-nav-lateral`   only enterable with left/right (side rails), so up/down in
 *                        content never jumps into the nav.
 * - `data-nav-top`       the top bar: a separate layer over the page. Left/right stay inside it,
 *                        Down enters the page below it, and Up from the page only reaches it
 *                        when nothing in the page is above.
 * - `data-nav-skip`      excluded from navigation.
 * - `data-autofocus`     preferred landing spot when nothing is focused.
 *
 * Replaces `@noriginmedia/norigin-spatial-navigation`, which is React-hook based
 * and requires `react` at runtime.
 */

import { exitApp, onRemoteKey, type RemoteKeyEvent } from "./platform/keys";
import { reelShell } from "./platform/detect";

export type Direction = "up" | "down" | "left" | "right";
export type MediaKey = "playpause" | "play" | "pause" | "stop" | "ff" | "rw" | "next" | "prev";

const FOCUSABLE =
  'a[href],button,input,select,textarea,summary,[data-focusable],[tabindex]:not([tabindex="-1"])';

const KEY_DIR: Record<string, Direction> = {
  ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right",
  Up: "up", Down: "down", Left: "left", Right: "right",
};
const KEYCODE_DIR: Record<number, Direction> = { 37: "left", 38: "up", 39: "right", 40: "down" };
export interface TvNavigationOptions {
  /** Force TV behaviour (always-visible focus, pointer hover focuses, instant scroll). */
  tv?: boolean;
  debug?: boolean;
  /** Called when Back is pressed and no handler consumed it (default: `exitApp()` on webOS and Tizen). */
  onBack?: () => void;
}

interface State {
  tv: boolean;
  debug: boolean;
  onBack?: () => void;
  lastMove: number;
}

let state: State | undefined;
const groupMemory = new WeakMap<Element, HTMLElement>();
const backStack: Array<() => boolean | void> = [];
const mediaHandlers = new Set<(k: MediaKey, e: KeyboardEvent) => void>();

// ---------------------------------------------------------------------------
// Public API

/**
 * 10-foot user agents. Inside the Reel Web Android shell only its ` (TV)` token counts, so the Android TV
 * build gets the TV UI and the phone build never does; plain Android TV browsers keep the vendor rules.
 */
export function isTvUserAgent(ua = typeof navigator !== "undefined" ? navigator.userAgent : ""): boolean {
  const shell = reelShell(ua);
  if (shell) return shell.tv;
  return /Web0S|webOS|Tizen|SMART-TV|SmartTV|NetCast|BRAVIA|HbbTV|AFT[A-Z]/i.test(ua);
}

/** Initialise D-pad / remote spatial navigation (idempotent). */
export function initTvNavigation(opts: TvNavigationOptions = {}): void {
  if (state) {
    state.tv = opts.tv ?? state.tv;
    state.onBack = opts.onBack ?? state.onBack;
    return;
  }
  state = { tv: !!opts.tv, debug: !!opts.debug, onBack: opts.onBack, lastMove: 0 };
  const root = document.documentElement;
  root.setAttribute("data-input", state.tv ? "key" : "pointer");
  // Back + media keys come through the platform layer's normaliser (webOS 461,
  // Tizen 10009/registered media keys, browser Escape/Backspace). Digits and
  // channel up/down are left unhandled — reserved for Live TV zapping.
  onRemoteKey(onRemote);
  document.addEventListener("keydown", onKeyDown, true);
  document.addEventListener("focusin", onFocusIn);
  let lx = -1, ly = -1;
  document.addEventListener("mousemove", (e) => {
    // Some TVs emit synthetic mousemoves at a fixed point; require real motion.
    if (e.clientX === lx && e.clientY === ly) return;
    lx = e.clientX; ly = e.clientY;
    root.setAttribute("data-input", "pointer");
  }, { passive: true });
  document.addEventListener("mouseover", (e) => {
    // LG Magic Remote: the pointer *is* focus.
    if (!state?.tv) return;
    const el = closestFocusable(e.target as Element | null);
    if (el && el !== document.activeElement) el.focus({ preventScroll: true });
  }, { passive: true });
}

export function isTvMode(): boolean {
  return !!state?.tv;
}

/** Move focus in a direction. Returns false when there is nowhere to go. */
export function moveFocus(dir: Direction, opts: { repeat?: boolean } = {}): boolean {
  const scope = activeScope();
  const cur = document.activeElement as HTMLElement | null;
  if (!cur || cur === document.body || !scope.contains(cur) || !isVisible(cur)) {
    // Lost focus (content re-rendered): resume in the page, not the side rail.
    const main = scope === document.body ? document.querySelector("main") : null;
    return (main && focusFirst(main)) || focusFirst(scope);
  }
  const target = findNext(cur, dir, scope);
  if (!target) return false;
  focusElement(target, { smooth: !opts.repeat });
  return true;
}

/** Focus `[data-autofocus]` (or the first focusable) inside `container`. */
export function focusFirst(container: ParentNode = activeScope()): boolean {
  const auto = container.querySelector<HTMLElement>("[data-autofocus]");
  const el = auto && isCandidate(auto) ? auto : candidates(container)[0];
  if (!el) return false;
  focusElement(el, { smooth: false });
  return true;
}

/** Focus an element and keep it in view (row-aware). */
export function focusElement(el: HTMLElement, opts: { smooth?: boolean } = {}): void {
  try {
    el.focus({ preventScroll: true });
  } catch {
    el.focus();
  }
  scrollIntoViewSmart(el, opts.smooth ?? true);
}

/** Restore focus to the element carrying `data-fid="{id}"`, if rendered. */
export function focusById(id: string | number, container: ParentNode = document): boolean {
  const el = container.querySelector<HTMLElement>(`[data-fid="${String(id).replace(/"/g, "")}"]`);
  if (!el || !isVisible(el)) return false;
  focusElement(el, { smooth: false });
  return true;
}

/**
 * Register a Back handler (LIFO). Return `true` from the handler to consume the
 * press. Returns a disposer.
 */
export function onBack(handler: () => boolean | void): () => void {
  backStack.push(handler);
  return () => {
    const i = backStack.lastIndexOf(handler);
    if (i >= 0) backStack.splice(i, 1);
  };
}

/** Subscribe to remote media keys (play/pause/ff/rw…). Returns a disposer. */
export function onMediaKey(handler: (k: MediaKey, e: KeyboardEvent) => void): () => void {
  mediaHandlers.add(handler);
  return () => mediaHandlers.delete(handler);
}

// ---------------------------------------------------------------------------
// Key handling

function onKeyDown(e: KeyboardEvent): void {
  if (!state) return;
  const code = e.keyCode || e.which;
  const dir = KEY_DIR[e.key] ?? KEYCODE_DIR[code];
  const active = document.activeElement as HTMLElement | null;
  const typing = isTextInput(active);

  if (e.key !== "Shift" && e.key !== "Control" && e.key !== "Alt" && e.key !== "Meta") {
    document.documentElement.setAttribute("data-input", "key");
  }

  if (dir) {
    if (e.defaultPrevented) return;
    if (typing && !caretAtEdge(active as HTMLInputElement, dir)) return;
    // Elements may claim arrows for themselves (e.g. a seek bar).
    if (active?.closest(`[data-nav-capture~="${dir}"]`)) return;
    e.preventDefault();
    const now = Date.now();
    // Throttle auto-repeat so slow TV SoCs don't queue up layout work.
    if (e.repeat && now - state.lastMove < 70) return;
    state.lastMove = now;
    moveFocus(dir, { repeat: e.repeat });
    return;
  }

  if ((e.key === "Enter" || code === 13) && active && active.hasAttribute("data-focusable") &&
      !/^(A|BUTTON|INPUT|SELECT|TEXTAREA)$/.test(active.tagName)) {
    e.preventDefault();
    active.click();
  }
}

const MEDIA: Partial<Record<RemoteKeyEvent["key"], MediaKey>> = {
  play: "play", pause: "pause", playpause: "playpause", stop: "stop", ff: "ff", rw: "rw",
};

function onRemote(e: RemoteKeyEvent): boolean {
  if (!state) return false;
  const media = MEDIA[e.key];
  if (media) {
    if (!mediaHandlers.size) return false;
    mediaHandlers.forEach((h) => h(media, e.original));
    return true;
  }
  if (e.key !== "back") return false;
  const active = document.activeElement as HTMLElement | null;
  // Escape in a text field just leaves the field.
  if (isTextInput(active) && e.original.key === "Escape") {
    active!.blur();
    return true;
  }
  for (let i = backStack.length - 1; i >= 0; i--) {
    if (backStack[i]!() === true) return true;
  }
  if (state.onBack) {
    state.onBack();
    return true;
  }
  // Root screen: leave the app on webOS and Tizen. Elsewhere Back stays unhandled, so a browser does
  // nothing and the Reel Web Android shell leaves (it only acts on an Escape the page didn't take).
  return state.tv && exitApp();
}

function onFocusIn(e: FocusEvent): void {
  const el = e.target as HTMLElement;
  if (!el || !el.closest) return;
  let g = el.closest("[data-nav-group]");
  while (g) {
    groupMemory.set(g, el);
    g = g.parentElement ? g.parentElement.closest("[data-nav-group]") : null;
  }
}

function isTextInput(el: Element | null): boolean {
  if (!el) return false;
  if (el.tagName === "TEXTAREA") return true;
  if (el.tagName !== "INPUT") return false;
  const t = (el as HTMLInputElement).type;
  return /^(text|search|email|url|password|tel|number)$/.test(t);
}

function caretAtEdge(input: HTMLInputElement, dir: Direction): boolean {
  if (dir === "up" || dir === "down") return true;
  let start: number | null = null;
  try {
    start = input.selectionStart;
  } catch {
    return true;
  }
  if (start == null) return true;
  return dir === "left" ? start === 0 && input.selectionEnd === 0 : start === input.value.length;
}

// ---------------------------------------------------------------------------
// Geometry

function activeScope(): Element {
  const traps = document.querySelectorAll("[data-nav-trap]");
  return traps.length ? traps[traps.length - 1]! : document.body;
}

function isVisible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  if (r.width === 0 && r.height === 0) return false;
  // offsetParent is null for display:none ancestors (and for position:fixed).
  if (el.offsetParent === null && getComputedStyle(el).position !== "fixed") {
    let p: HTMLElement | null = el.parentElement;
    while (p && p !== document.body) {
      if (getComputedStyle(p).position === "fixed") break;
      p = p.parentElement;
    }
    if (!p || p === document.body) return false;
  }
  return true;
}

function isCandidate(el: HTMLElement): boolean {
  if ((el as HTMLButtonElement).disabled) return false;
  if (el.closest("[data-nav-skip],[inert],[aria-hidden='true']")) return false;
  if (el.getAttribute("tabindex") === "-1" && !el.hasAttribute("data-focusable")) return false;
  return isVisible(el);
}

function candidates(scope: ParentNode): HTMLElement[] {
  const out: HTMLElement[] = [];
  const list = scope.querySelectorAll<HTMLElement>(FOCUSABLE);
  for (let i = 0; i < list.length; i++) if (isCandidate(list[i]!)) out.push(list[i]!);
  return out;
}

function closestFocusable(el: Element | null): HTMLElement | null {
  const f = el && el.closest ? (el.closest(FOCUSABLE) as HTMLElement | null) : null;
  return f && isCandidate(f) && activeScope().contains(f) ? f : null;
}

interface Box { l: number; t: number; r: number; b: number; cx: number; cy: number }

function box(el: Element): Box {
  const r = el.getBoundingClientRect();
  return { l: r.left, t: r.top, r: r.right, b: r.bottom, cx: r.left + r.width / 2, cy: r.top + r.height / 2 };
}

/** Rect of `el` without the focus scale transform, so neighbours don't overlap. */
function unscaledBox(el: HTMLElement): Box {
  const b = box(el);
  const s = parseFloat(getComputedStyle(el).getPropertyValue("--nav-scale")) || 1;
  if (s === 1) return b;
  const hw = (b.r - b.l) / s / 2, hh = (b.b - b.t) / s / 2;
  return { l: b.cx - hw, r: b.cx + hw, t: b.cy - hh, b: b.cy + hh, cx: b.cx, cy: b.cy };
}

function score(from: Box, to: Box, dir: Direction): number {
  const vertical = dir === "up" || dir === "down";
  let primary: number;
  switch (dir) {
    case "right": if (to.cx <= from.cx + 1 || to.r <= from.r) return -1; primary = to.l - from.r; break;
    case "left": if (to.cx >= from.cx - 1 || to.l >= from.l) return -1; primary = from.l - to.r; break;
    case "down": if (to.cy <= from.cy + 1 || to.b <= from.b) return -1; primary = to.t - from.b; break;
    case "up": if (to.cy >= from.cy - 1 || to.t >= from.t) return -1; primary = from.t - to.b; break;
  }
  primary = Math.max(0, primary);
  // Orthogonal: 0 when the ranges overlap, otherwise the gap; plus a light centre bias.
  const gap = vertical
    ? Math.max(0, to.l - from.r, from.l - to.r)
    : Math.max(0, to.t - from.b, from.t - to.b);
  const centre = vertical ? Math.abs(to.cx - from.cx) : Math.abs(to.cy - from.cy);
  return primary + gap * 3 + centre * 0.3;
}

function findNext(cur: HTMLElement, dir: Direction, scope: Element): HTMLElement | null {
  const from = unscaledBox(cur);
  const vertical = dir === "up" || dir === "down";
  const curLateral = cur.closest("[data-nav-lateral]");
  const all = candidates(scope);
  const layers = scope === document.body ? splitTopLayer(cur, all, dir, from) : [all];
  let list = all;
  let best: HTMLElement | null = null;
  for (const layer of layers) {
    list = layer;
    let bestScore = Infinity;
    for (const el of layer) {
      if (el === cur || el.contains(cur)) continue;
      if (vertical && el.closest("[data-nav-lateral]") !== curLateral) continue;
      const s = score(from, box(el), dir);
      if (s >= 0 && s < bestScore) {
        bestScore = s;
        best = el;
      }
    }
    if (best) break;
  }
  // Left/right: stay on the same visual line when anything on it qualifies,
  // instead of sliding diagonally into the row above/below.
  if (best && !vertical) {
    let lineBest: HTMLElement | null = null;
    let lineScore = Infinity;
    for (const el of list) {
      if (el === cur || el.contains(cur)) continue;
      const b = box(el);
      if (b.b <= from.t + 2 || b.t >= from.b - 2) continue;
      const s = score(from, b, dir);
      if (s >= 0 && s < lineScore) {
        lineScore = s;
        lineBest = el;
      }
    }
    if (lineBest) best = lineBest;
    // Nothing on this line: only hop off-line into/out of a side rail (end of a row stays put).
    else if (!best.closest("[data-nav-lateral]") && !curLateral) best = null;
  }
  if (!best) return null;
  return restoreFromGroup(cur, best, dir, from) ?? best;
}

/**
 * The candidate lists to try, in order, around the fixed top bar. Page content scrolls behind the
 * bar, so geometry alone would pick a tile hidden under it, or jump to the bar from a row whose
 * neighbour above is further off to one side.
 */
function splitTopLayer(cur: HTMLElement, all: HTMLElement[], dir: Direction, from: Box): HTMLElement[][] {
  const curTop = cur.closest("[data-nav-top]");
  const inBar = all.filter((el) => !!el.closest("[data-nav-top]"));
  const inPage = all.filter((el) => !el.closest("[data-nav-top]"));
  if (dir === "left" || dir === "right") return [curTop ? inBar : inPage];
  if (!curTop) return dir === "up" ? [inPage, inBar] : [inPage];
  if (dir === "up") return [];
  // Down from the bar: prefer what is fully below it.
  return [inPage.filter((el) => box(el).t >= from.b - 2), inPage];
}

/** Entering a group from outside lands on its remembered (or default) child. */
function restoreFromGroup(cur: HTMLElement, best: HTMLElement, dir: Direction, from: Box): HTMLElement | null {
  let outer: Element | null = null;
  let g: Element | null = best.closest("[data-nav-group]");
  while (g) {
    if (!g.contains(cur)) outer = g;
    g = g.parentElement ? g.parentElement.closest("[data-nav-group]") : null;
  }
  if (!outer || outer.getAttribute("data-nav-group") === "free") return null;
  const remembered = groupMemory.get(outer);
  const pick = (el: HTMLElement | null | undefined) =>
    el && el.isConnected && outer!.contains(el) && isCandidate(el) && score(from, box(el), dir) >= 0 ? el : null;
  return (
    pick(remembered) ??
    pick(outer.querySelector<HTMLElement>("[data-nav-default]")) ??
    pick(outer.querySelector<HTMLElement>('[aria-current="page"]'))
  );
}

// ---------------------------------------------------------------------------
// Scrolling

function scrollIntoViewSmart(el: HTMLElement, smooth: boolean): void {
  const behavior: ScrollBehavior = smooth && !(state?.tv) ? "smooth" : "auto";
  const tv = !!state?.tv;

  // Horizontal scroller (rows).
  const sx = el.closest<HTMLElement>('[data-nav-scroll="x"]');
  if (sx) {
    const pad = parseFloat(getComputedStyle(sx).paddingLeft) || 0;
    const r = el.getBoundingClientRect();
    // Where the tile rests, not where a slide has it right now: a row parting for the preview card
    // (#301) moves tiles with translateX and is closing up as focus moves on.
    const tx = currentTranslateX(el);
    const er = { left: r.left - tx, right: r.right - tx };
    const sr = sx.getBoundingClientRect();
    let delta = 0;
    if (er.left < sr.left + pad) delta = er.left - sr.left - pad;
    else if (er.right > sr.right - pad) delta = er.right - sr.right + pad;
    if (delta) scrollBy(sx, delta, 0, behavior);
  }

  // Inner vertical scroller (e.g. side sheets, episode lists).
  const sy = el.closest<HTMLElement>('[data-nav-scroll="y"]');
  if (sy) {
    const er = el.getBoundingClientRect();
    const sr = sy.getBoundingClientRect();
    const m = Math.min(80, sr.height * 0.15);
    let delta = 0;
    if (er.top < sr.top + m) delta = er.top - sr.top - m;
    else if (er.bottom > sr.bottom - m) delta = er.bottom - sr.bottom + m;
    if (delta) scrollBy(sy, 0, delta, behavior);
    return;
  }

  if (isInFixedLayer(el)) return;

  const vh = window.innerHeight;
  const anchor = el.closest<HTMLElement>("[data-nav-anchor]");
  if (anchor && tv) {
    const topVar = getComputedStyle(anchor).getPropertyValue("--nav-anchor-top").trim();
    const top = topVar.endsWith("vh") ? (parseFloat(topVar) / 100) * vh : parseFloat(topVar) || vh * 0.3;
    const ar = anchor.getBoundingClientRect();
    const wanted = window.pageYOffset + ar.top - top;
    window.scrollTo({ top: Math.max(0, wanted), behavior });
    return;
  }
  const r = el.getBoundingClientRect();
  const margin = Math.min(vh * 0.18, 160);
  if (r.top < margin) window.scrollTo({ top: Math.max(0, window.pageYOffset + r.top - margin), behavior });
  else if (r.bottom > vh - margin)
    window.scrollTo({ top: window.pageYOffset + r.bottom - vh + margin, behavior });
}

/** The x offset of an element's own computed transform, mid-transition included (0 for none). */
export function currentTranslateX(el: Element): number {
  const m = /^matrix(3d)?\(([^)]*)\)$/.exec(getComputedStyle(el).transform || "");
  if (!m) return 0;
  const v = m[2]!.split(",");
  const x = parseFloat((m[1] ? v[12] : v[4]) ?? "");
  return isFinite(x) ? x : 0;
}

function scrollBy(el: HTMLElement, dx: number, dy: number, behavior: ScrollBehavior): void {
  if (typeof el.scrollBy === "function") el.scrollBy({ left: dx, top: dy, behavior });
  else {
    el.scrollLeft += dx;
    el.scrollTop += dy;
  }
}

function isInFixedLayer(el: HTMLElement): boolean {
  let p: HTMLElement | null = el;
  while (p && p !== document.body) {
    if (p.hasAttribute("data-nav-fixed")) return true;
    p = p.parentElement;
  }
  return false;
}

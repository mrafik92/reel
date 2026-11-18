// Keep navigation and its data loading independent of the animation. The copy
// only exists between two rendered endpoints and never takes focus or input.
interface Snapshot {
  image: HTMLImageElement;
  rect: DOMRect;
}

let source: { from: string; to: string; art: Snapshot; entered?: boolean } | undefined;
let returning: { path: string; art: Snapshot; to: string } | undefined;
let flight: Animation | undefined;

const address = () => location.pathname + location.search + location.hash;
const allowsMotion = () =>
  typeof Element.prototype.animate === "function" &&
  !matchMedia("(prefers-reduced-motion: reduce)").matches;

function snapshot(image: HTMLImageElement | null | undefined): Snapshot | undefined {
  if (!image?.complete || !image.naturalWidth) return;
  const rect = image.getBoundingClientRect();
  if (!rect.width || !rect.height || rect.bottom <= 0 || rect.top >= innerHeight ||
      rect.right <= 0 || rect.left >= innerWidth) return;
  return { image, rect };
}

export function rememberPoster(event: MouseEvent): void {
  if (event.button || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  const link = (event.target as Element | null)?.closest?.<HTMLAnchorElement>("a.card--poster");
  if (!link || link.target || !allowsMotion()) return;
  const url = new URL(link.href);
  if (url.origin !== location.origin || !/^\/item\/\d+$/.test(url.hash.slice(1) || url.pathname)) return;
  returning = undefined;
  const art = snapshot(link.querySelector<HTMLImageElement>(".card__img img"));
  source = art ? { from: address(), to: link.href, art } : undefined;
}

function fly(from: Snapshot, to: Snapshot): void {
  if (!allowsMotion()) return;
  flight?.cancel();
  const copy = document.createElement("img");
  copy.src = from.image.currentSrc || from.image.src;
  copy.alt = "";
  copy.setAttribute("aria-hidden", "true");
  const a = from.rect;
  const b = to.rect;
  Object.assign(copy.style, {
    position: "fixed", top: b.top + "px", left: b.left + "px", width: b.width + "px",
    height: b.height + "px", objectFit: "cover", borderRadius: "0.45rem", zIndex: "80",
    pointerEvents: "none", transformOrigin: "top left",
  });
  document.body.append(copy);
  const animation = copy.animate([
    { transform: `translate(${a.left - b.left}px, ${a.top - b.top}px) scale(${a.width / b.width}, ${a.height / b.height})` },
    { transform: "none", offset: 0.88 },
    { transform: "none", opacity: 0 },
  ], { duration: 300, easing: "cubic-bezier(.2,.75,.3,1)" });
  flight = animation;
  const remove = () => {
    copy.remove();
    if (flight === animation) flight = undefined;
  };
  animation.onfinish = animation.oncancel = remove;
}

export function enterPoster(poster: HTMLElement): void {
  if (source?.to !== location.href || source.entered) return;
  const target = snapshot(poster.querySelector("img"));
  if (!target) return;
  source.entered = true;
  fly(source.art, target);
}

export function leavePoster(poster: HTMLElement | undefined, id: number): void {
  flight?.cancel();
  if (!source || source.to.split("/").pop() !== String(id)) return;
  const art = source.entered && snapshot(poster?.querySelector("img"));
  if (art) returning = { path: source.from, to: source.to, art };
  source = undefined;
}

export function returnPoster(): void {
  const move = returning;
  returning = undefined;
  if (!move || move.path !== address()) return;
  const link = document.activeElement?.closest<HTMLAnchorElement>("a.card--poster");
  if (link?.href !== move.to) return;
  const target = snapshot(link.querySelector<HTMLImageElement>(".card__img img"));
  if (target) fly(move.art, target);
}

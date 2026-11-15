import { createEffect, createRoot, createSignal, on, type Accessor } from "solid-js";

/**
 * Where overlays (sheets, toasts) mount. In full screen the browser only paints the
 * full-screen element's subtree, so a Portal into `<body>` would open invisibly (#94).
 * While something is full screen, overlays mount inside it; otherwise into `<body>`.
 */

type FsDoc = Pick<Document, "body"> & {
  fullscreenElement?: Element | null;
  webkitFullscreenElement?: Element | null;
};

/** The element overlays should mount into for this document's current full-screen state. */
export function overlayTarget(doc: FsDoc): Element {
  return doc.fullscreenElement ?? doc.webkitFullscreenElement ?? doc.body;
}

let mount: Accessor<Element> | undefined;

/**
 * Reactive overlay mount point: pass it to `<Portal mount={overlayMount()}>`. Solid's Portal
 * re-mounts when it changes, so an overlay open while entering or leaving full screen moves along.
 */
export function overlayMount(): Element {
  if (typeof document === "undefined") return undefined as unknown as Element;
  if (!mount) {
    mount = createRoot(() => {
      const [el, setEl] = createSignal<Element>(overlayTarget(document), { equals: false });
      const sync = () => setEl(overlayTarget(document));
      document.addEventListener("fullscreenchange", sync);
      document.addEventListener("webkitfullscreenchange", sync);
      return el;
    });
  }
  return mount();
}

/**
 * Moving a Portal (entering or leaving full screen with a sheet open) drops focus to `<body>`,
 * which strands D-pad and keyboard users. Call from a sheet: puts focus back inside `panel`.
 */
export function keepFocusOnOverlayMove(panel: () => HTMLElement | undefined, focus: (panel: HTMLElement) => void): void {
  createEffect(
    on(overlayMount, () => {
      requestAnimationFrame(() => {
        const p = panel();
        if (p && p.isConnected && !p.contains(document.activeElement)) focus(p);
      });
    }, { defer: true }),
  );
}

import { createSignal } from "solid-js";

export type ToastTone = "info" | "success" | "error";
export interface Toast {
  id: number;
  tone: ToastTone;
  text: string;
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
export { toasts };
let n = 0;

/** Brief, non-blocking message (top-right; announced politely to screen readers). */
export function toast(text: string, tone: ToastTone = "info", ms = 4200): void {
  const id = ++n;
  setToasts((t) => t.slice(-2).concat({ id, tone, text }));
  setTimeout(() => dismiss(id), ms);
}
export function dismiss(id: number): void {
  setToasts((t) => t.filter((x) => x.id !== id));
}

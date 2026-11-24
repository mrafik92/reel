import { createRoot, createSignal, type Accessor } from "solid-js";

/** True at phone width (<= 760 px), where tiles keep their caption and there is no preview card. */
export const isPhoneWidth: Accessor<boolean> = createRoot(() => {
  const mq = window.matchMedia("(max-width: 760px)");
  const [phone, setPhone] = createSignal(mq.matches);
  // addListener: MediaQueryList has no addEventListener on the oldest TV browsers.
  mq.addListener((e) => setPhone(e.matches));
  return phone;
});

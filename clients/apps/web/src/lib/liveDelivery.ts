import { createSignal } from "solid-js";
import { currentUser } from "./auth";

/**
 * How this device plays Live TV: through the Reel server (default: shared upstream, fast zapping,
 * the provider login stays on the server) or straight from the provider. Per device, so it lives in
 * localStorage; direct also needs the server to allow it (`/me` → `direct_live`).
 */
export type LiveDelivery = "server" | "direct";

const KEY = "reel.live.delivery";

function read(): LiveDelivery {
  try {
    return localStorage.getItem(KEY) === "direct" ? "direct" : "server";
  } catch {
    return "server";
  }
}

const [delivery, setDelivery] = createSignal<LiveDelivery>(read());

/** What the user chose on this device. */
export { delivery as liveDelivery };

export function chooseLiveDelivery(v: LiveDelivery): void {
  setDelivery(v);
  try {
    if (v === "direct") localStorage.setItem(KEY, v);
    else localStorage.removeItem(KEY);
  } catch {
    /* storage disabled: the choice lasts until reload */
  }
}

/** Does the server let this user play direct? */
export const directAllowed = (): boolean => !!currentUser()?.direct_live;

/** Direct playback is chosen here *and* allowed by the server. */
export const wantsDirect = (): boolean => delivery() === "direct" && directAllowed();

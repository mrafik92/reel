/**
 * The automatic retune after Live TV comes back from the background (#248).
 *
 * On Android the app's network can still be blocked for a moment after it returns to the
 * foreground, so the first tune request fails without an answer ("Can't reach Reel"). That tune
 * gets exactly one more try after the network is back (or a short wait). Only a failure with no
 * answer at all is retried: a server or provider answer (429, 409, 503…) shows at once, and so
 * does a second network failure. The provider allows one connection and rate-limits, so this
 * never loops.
 */
import { ApiError } from "../../api/http";

/** No answer arrived because the connection failed or the browser was offline (not a cancel, not our 60 s timeout). */
export function isResumeNetworkFailure(e: unknown): boolean {
  return e instanceof ApiError && e.status === 0 && (e.network === "network" || e.network === "offline");
}

/** Longest wait for `online` when the browser says it's offline. */
export const ONLINE_CAP_MS = 8_000;
/** Pause before the retry when the browser already says it's online (Android's block isn't visible to `navigator.onLine`). */
export const SETTLE_MS = 1_500;

/** Resolves once the network looks usable again: on `online` (at most `capMs`), or after `settleMs` when already online. */
export function waitForNetwork(o: { capMs?: number; settleMs?: number } = {}): Promise<void> {
  const cap = o.capMs ?? ONLINE_CAP_MS;
  const settle = o.settleMs ?? SETTLE_MS;
  const offline = typeof navigator !== "undefined" && navigator.onLine === false;
  if (!offline || typeof window === "undefined" || typeof window.addEventListener !== "function")
    return new Promise((r) => setTimeout(r, settle));
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      window.removeEventListener("online", done);
      resolve();
    };
    const timer = setTimeout(done, cap);
    window.addEventListener("online", done);
  });
}

/**
 * Run `send`; on a network failure wait for the network and run it once more. Anything else
 * (success, a server error, a second failure) goes straight back to the caller. `still()` is
 * checked after the wait: when the viewer zapped or left meanwhile, the first error is rethrown
 * without a second request.
 */
export async function withResumeRetry<T>(
  send: () => Promise<T>,
  o: { wait?: () => Promise<void>; still?: () => boolean; onWait?: () => void } = {},
): Promise<T> {
  try {
    return await send();
  } catch (e) {
    if (!isResumeNetworkFailure(e)) throw e;
    o.onWait?.();
    await (o.wait ?? waitForNetwork)();
    if (o.still && !o.still()) throw e;
    return send();
  }
}

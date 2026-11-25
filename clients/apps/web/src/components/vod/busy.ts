import { ApiError } from "../../api/http";
import { vod, type VodHolder } from "../../api/vod";
import { currentUser } from "../../lib/auth";
import type { PlayError } from "../../pages/Player";

/**
 * Who holds the provider. Two shapes: `{ holder }` (VOD contract) or the Live TV budget policy's
 * admin 409 `{ takeover: true, holders: [{ user_id, username, session_id, channel_name, … }] }`.
 */
function holdersOf(e: ApiError): VodHolder[] {
  const b = e.body ?? {};
  const one = b.holder as VodHolder | undefined;
  if (one && typeof one === "object") return [one];
  const many = b.holders as (VodHolder & { channel_name?: string | null })[] | undefined;
  if (Array.isArray(many))
    return many.map((h) => ({ ...h, title: h.title ?? h.channel_name ?? null, kind: h.kind ?? (h.channel_name ? "live" : null) }));
  return [];
}

/** Is the stream holding the provider one of the viewer's own (another screen of theirs)? */
function isMine(h: VodHolder | undefined, msg: string): boolean {
  if (h?.mine != null) return !!h.mine;
  const me = currentUser();
  if (h && me && (h.user_id === me.id || (h.username && h.username === me.username))) return true;
  return !h && /\byou(?:'re| are)\b|\byour other\b/i.test(msg);
}

const describe = (h: VodHolder) => {
  const what = h.title ? `${h.kind === "live" ? "the channel " : ""}${h.title}` : "something";
  return `${what}${h.device ? ` on ${h.device}` : ""}`;
};

/**
 * Friendly wording for IPTV playback failures. Providers usually allow a single connection, so
 * "busy" is the common case:
 * - admin 409 `{ takeover: true }` → name who's watching, offer **Take over**
 * - the viewer's own other screen → offer **Stop the other stream**
 * - anyone else → a plain explanation (non-admins aren't told who)
 */
export function vodPlayError(e: unknown, o: { takeover?: () => void } = {}): PlayError | undefined {
  if (!(e instanceof ApiError)) return undefined;
  const busy = e.status === 429 || (e.status === 409 && !!e.body?.takeover);
  if (busy) {
    const hs = holdersOf(e);
    const h = hs[0];
    const msg = e.message && e.message !== "busy" ? e.message : "";
    if (isMine(h, msg)) {
      const canStop = !!h?.session_id;
      return {
        title: "You’re already watching IPTV on another screen",
        body:
          (h ? `You’re watching ${describe(h)}. ` : msg ? msg.replace(/\.?$/, ". ") : "") +
          "Your provider only allows one stream at a time" + (canStop ? " — stop that one and this starts right away." : ". Stop it there, then try again."),
        retry: true,
        action: canStop ? { label: "Stop the other stream", run: () => vod.stopHolder(h!) } : undefined,
      };
    }
    if (e.status === 409 && o.takeover) {
      const who = h?.username ?? "Someone else";
      const others = hs.length > 1 ? ` (and ${hs.length - 1} more)` : "";
      return {
        title: "Your IPTV connection is in use",
        body: `${who}${others} is watching ${h ? describe(h) : "something"}. Your provider only allows ${hs.length || 1} stream${hs.length > 1 ? "s" : ""} at a time — take over to stop theirs and start this.`,
        retry: true,
        action: { label: "Take over", run: async () => o.takeover!() },
      };
    }
    // The server's anonymous wording ("All 1 connection(s) … in use") reads like a log line: say it plainly.
    const named = msg && !/connection\(s\)|in use by other viewers/i.test(msg) ? msg.replace(/\.?$/, ". ") : "";
    return {
      title: "Your IPTV connection is in use",
      body:
        (h?.username ? `${h.username} is watching ${describe(h)}. ` : named || "Someone else in your household is watching IPTV right now. ") +
        "Your provider only allows one stream at a time, so this can start once they stop" +
        (e.retryAfter ? ` — or try again in ${Math.max(1, Math.ceil(e.retryAfter / 60))} min.` : "."),
      retry: true,
    };
  }
  if (e.status === 404) return { title: "No longer on your provider", body: "Your IPTV provider removed this title. It may come back after the next catalogue refresh.", retry: false };
  if (e.status === 502 || e.status === 503 || e.status === 504)
    return { title: "Your IPTV provider didn’t answer", body: `${e.message.replace(/\.?$/, ".")} Providers drop out now and then — try again in a moment.`, retry: true };
  if (e.status === 0) return { title: "Can’t reach Reel", body: "Check your connection and try again.", retry: true };
  return { title: "Couldn’t start this title", body: e.message, retry: true };
}

import { iptv, type IptvChannel, type TuneBody } from "../../api/iptv";
import type { CastSource } from "./index";

/**
 * Cast source for a live channel — for the Live TV page: hand it to `startCast()` and show
 * `<CastRemote>` (it hides seeking for live). `POST /iptv/channels/{id}/tune` with
 * `for_cast: true` returns an absolute, long-lived URL the receiver can fetch; DLNA TVs get
 * the channel pushed by the server.
 *
 *   const src = channelCastSource(ch, { eyebrow: nowPlayingTitle });
 *   await startCast(target, src, { audio: null, subtitle: -1, start: 0 });
 */
export function channelCastSource(ch: Pick<IptvChannel, "id" | "name" | "logo_url" | "number">, o: { eyebrow?: string } = {}): CastSource {
  return {
    meta: {
      key: `channel:${ch.id}`,
      title: ch.name,
      eyebrow: o.eyebrow ?? (ch.number != null ? `Channel ${ch.number}` : "Live TV"),
      poster: ch.logo_url,
      live: true,
    },
    open: async (target) => {
      const body = { preset: target, for_cast: true } as TuneBody & { for_cast: boolean };
      const t = await iptv.tune(ch.id, body);
      const hls = t.hls || /\.m3u8(\?|$)/i.test(t.url);
      return {
        sessionId: t.session_id,
        url: t.url,
        contentType: hls ? "application/x-mpegURL" : "video/mp2t",
        fmp4: hls && t.mode !== "passthrough",
        live: true,
        audio: [],
        subtitles: [],
        // Live sessions stay up while the receiver pulls segments; stop frees the provider slot.
        keepAlive: () => undefined,
        release: () => void iptv.stop(t.session_id).catch(() => undefined),
      };
    },
    dlna: () => ({ kind: "channel", id: ch.id }),
  };
}

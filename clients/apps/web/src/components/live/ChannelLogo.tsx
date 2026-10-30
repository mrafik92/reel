import { createSignal, Show, type JSX } from "solid-js";
import { mediaUrl } from "@reel/ui";
import type { IptvChannel } from "../../api/iptv";

/** Channel logo on a dark tile; falls back to the channel's initials. */
export function ChannelLogo(props: { channel: IptvChannel; class?: string }): JSX.Element {
  const [failed, setFailed] = createSignal(false);
  const initials = () => {
    const w = props.channel.name.replace(/[^\p{L}\p{N} ]/gu, "").split(/\s+/).filter(Boolean);
    return (w.length > 1 ? w[0]!.charAt(0) + w[1]!.charAt(0) : props.channel.name.slice(0, 3)).toUpperCase();
  };
  return (
    <span class={"chlogo" + (props.class ? " " + props.class : "")} aria-hidden="true">
      <Show when={props.channel.logo_url && !failed()} fallback={<span class="chlogo__text">{initials()}</span>}>
        <img src={mediaUrl(props.channel.logo_url)} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />
      </Show>
    </span>
  );
}

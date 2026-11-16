import { createResource, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { formatClock, Icon } from "@reel/ui";
import { api, type AdminSession } from "../../api";
import { MODE_LABEL } from "../../lib/playback";
import { toast } from "../../lib/toast";
import { ConfirmButton, Panel, timeAgo } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

export default function Sessions(): JSX.Element {
  const [list, { refetch, mutate }] = createResource(() => api.sessions());
  onMount(() => {
    const t = setInterval(() => void refetch(), 5000);
    onCleanup(() => clearInterval(t));
  });
  const kill = async (s: AdminSession) => {
    try {
      await api.killSession(s.id);
      mutate((l) => (l ?? []).filter((x) => x.id !== s.id));
      toast(`Stopped ${s.username ?? "user"}'s stream`);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't stop the stream", "error");
    }
  };
  return (
    <Panel title="Now playing" icon="activity" aside={<span class="panel__live"><i /> Live · refreshes every 5s</span>}>
      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={refetch} />}>
        <ul class="list">
          <For each={list()} fallback={<li class="panel__empty">Nobody is watching right now.</li>}>
            {(s) => (
              <li class="list__row list__row--stack">
                <div class="list__line">
                  <span class="avatar" aria-hidden="true">{(s.username ?? "?").charAt(0).toUpperCase()}</span>
                  <div class="list__main">
                    <strong>{s.item_title ?? `File #${s.file_id}`}</strong>
                    <span class="list__sub">
                      {s.username ?? `user ${s.user_id}`} · {s.device_id ?? "unknown device"} · started {timeAgo(s.started_at)}
                      {s.active ? "" : " · idle"}
                    </span>
                  </div>
                  <div class="list__actions">
                    <span class={`pill pill--status pill--mode-${s.mode}`}>{MODE_LABEL[s.mode] ?? s.mode}</span>
                    <ConfirmButton label="Stop" icon="x" confirm="Stop this stream?" onConfirm={() => kill(s)} />
                  </div>
                </div>
                <div class="session__detail">
                  <Show when={s.progress != null}>
                    <div class="meter meter--thin"><i style={{ width: `${(s.progress ?? 0) * 100}%` }} /></div>
                  </Show>
                  <span>
                    {formatClock(s.position_s)}
                    {s.bitrate_kbps ? ` · ${(s.bitrate_kbps / 1000).toFixed(1)} Mbps` : ""}
                    {s.encoder ? ` · ${s.encoder}` : s.hwaccel && s.hwaccel !== "none" ? ` · ${s.hwaccel.toUpperCase()}` : ""}
                  </span>
                  <Show when={s.reasons.length}>
                    <ul class="session__reasons">
                      <For each={s.reasons}>{(r) => <li><Icon name="info" /> {r}</li>}</For>
                    </ul>
                  </Show>
                </div>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

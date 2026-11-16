import { createResource, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, type HwAccel } from "../../api";
import { Panel } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

const NAMES: Record<string, string> = {
  vaapi: "VA-API", qsv: "Intel Quick Sync", nvenc: "NVIDIA NVENC", video_toolbox: "VideoToolbox", software: "Software (CPU)",
};
const label = (h: HwAccel) => NAMES[h.type] ?? h.type;
const device = (h: HwAccel) => ("device" in h ? (h as { device: string }).device : "");
/** Low-power (fixed-function) VAAPI encoders that passed the startup test encode. */
const lowPower = (h: HwAccel) => {
  const lp = "low_power" in h ? h.low_power : undefined;
  const codecs = [lp?.h264 && "H.264", lp?.hevc && "HEVC"].filter(Boolean);
  return codecs.length ? codecs.join(", ") : "";
};

export default function Hardware(): JSX.Element {
  const [hw, { refetch }] = createResource(() => api.hwaccel());
  return (
    <Panel title="Transcoding" icon="chip">
      <Show when={!hw.error} fallback={<ErrorState error={hw.error} onRetry={refetch} />}>
        <Show when={hw()} fallback={<p class="panel__empty">Checking…</p>}>
          {(h) => (
            <>
              <div class="hw" tabindex="0" data-focusable>
                <span class="hw__badge" classList={{ "is-sw": h().selected.type === "software" }}>
                  <Icon name={h().selected.type === "software" ? "alert" : "check"} />
                </span>
                <div>
                  <p class="eyebrow">In use</p>
                  <strong class="hw__name">{label(h().selected)}</strong>
                  <Show when={device(h().selected)}><code>{device(h().selected)}</code></Show>
                  <Show when={h().selected.type === "software"}>
                    <p class="panel__lede">No GPU acceleration found — transcodes will use the CPU. On Intel, pass /dev/dri into the container.</p>
                  </Show>
                </div>
              </div>
              <dl class="kv">
                <dt>Available</dt>
                <dd>{h().available.map((a) => label(a) + (device(a) ? ` (${device(a)})` : "")).join(" · ") || "None"}</dd>
                <Show when={h().selected.type === "vaapi"}>
                  <dt>Low-power encoders</dt>
                  <dd>{lowPower(h().selected) || "Not used"}</dd>
                </Show>
                <dt>ffmpeg hwaccels</dt>
                <dd><For each={h().ffmpeg_hwaccels}>{(x) => <code>{x}</code>}</For></dd>
                <Show when={h().degraded?.length}>
                  <dt>Limitations</dt>
                  <dd><ul><For each={h().degraded}>{(d) => <li class="kv__warn"><Icon name="alert" /> {d}</li>}</For></ul></dd>
                </Show>
                <Show when={h().notes.length}>
                  <dt>Notes</dt>
                  <dd><ul><For each={h().notes}>{(n) => <li>{n}</li>}</For></ul></dd>
                </Show>
              </dl>
            </>
          )}
        </Show>
      </Show>
    </Panel>
  );
}

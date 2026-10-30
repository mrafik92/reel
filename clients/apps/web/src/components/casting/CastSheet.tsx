import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { focusElement, focusFirst, Icon, onBack, type IconName } from "@reel/ui";
import { ApiError } from "../../api/http";
import { casting, type CastDevice } from "../../api/casting";
import { castSdkStatus, castState, castUnavailableWhy, loadCastSdk } from "../../lib/casting/chromecast";
import { AIRPLAY_SUPPORTED, type CastTarget } from "../../lib/casting";
import "../../styles/casting.css";

/**
 * "Play on…" — every way to put this title on a TV: AirPlay (Safari), Chromecast / Google TV
 * (Chrome) and the DLNA TVs the server found on the network. Rows say plainly why they're
 * unavailable instead of disappearing.
 */
export function CastSheet(props: {
  /** Active target id ("airplay", "chromecast", DLNA id) or undefined when playing here. */
  current?: string;
  /** Safari: whether an AirPlay receiver is on the network (from the <video> element). */
  airplayAvailable?: boolean;
  /** AirPlay needs the player's <video>: off on pages without one. */
  airplay?: boolean;
  /** The title can be pushed to DLNA TVs. */
  dlna?: boolean;
  /** Offer "This device" (stop casting). */
  onLocal?: () => void;
  onPick: (t: CastTarget) => void;
  onClose: () => void;
  footer?: JSX.Element;
}): JSX.Element {
  let panel!: HTMLDivElement;
  let returnTo: HTMLElement | null = null;
  let picked = false;
  void loadCastSdk();

  const [devices, { refetch }] = createResource(
    () => props.dlna !== false,
    async () => casting.devices(),
  );
  const [refreshing, setRefreshing] = createSignal(false);
  const dlnaMissing = () => devices.error instanceof ApiError && (devices.error.status === 404 || devices.error.status === 405);

  const pick = (fn: () => void) => {
    if (picked) return;
    picked = true;
    props.onClose();
    // Same tick budget as Sheet: the user gesture survives a 0 ms timer (Chrome + Safari).
    setTimeout(fn, 0);
  };

  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => {
      const sel = panel.querySelector<HTMLElement>('[aria-checked="true"]:not([disabled])');
      if (sel) focusElement(sel, { smooth: false });
      else focusFirst(panel);
    });
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });

  const ccDetail = () => {
    const st = castSdkStatus();
    if (st === "idle" || st === "loading") return "Checking for Cast devices…";
    if (st === "unavailable") return castUnavailableWhy() || "Not available in this browser";
    const cs = castState();
    if (cs === "NO_DEVICES_AVAILABLE") return "No Cast devices found on this network";
    if (cs === "CONNECTED") return props.current === "chromecast" ? "Playing now" : "Connected — tap to play here";
    if (cs === "CONNECTING") return "Connecting…";
    return "Pick your Chromecast in the next step";
  };
  const ccEnabled = () => castSdkStatus() === "ready" && castState() !== "NO_DEVICES_AVAILABLE";

  return (
    <Portal>
      <div
        class="sheet"
        data-nav-trap
        data-nav-fixed
        onKeyDown={(e) => {
          if (e.key === "Escape" || e.key === "Esc") {
            e.preventDefault();
            e.stopPropagation();
            props.onClose();
          }
        }}
      >
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel" ref={panel} role="dialog" aria-modal="true" aria-label="Play on">
          <header class="sheet__head">
            <h2>Play on…</h2>
            <p class="sheet__desc">Send this to a TV. You can keep using this screen as the remote.</p>
          </header>
          <div class="sheet__list" role="radiogroup" data-nav-scroll="y">
            <Show when={props.onLocal}>
              <Row icon="film" title="This device" detail="Stop casting and keep watching here" checked={!props.current} onClick={() => pick(props.onLocal!)} />
            </Show>
            <Show when={AIRPLAY_SUPPORTED}>
              <Row
                icon="airplay"
                title="AirPlay"
                detail={
                  props.airplay === false
                    ? "Apple TV and AirPlay TVs — starts in the player"
                    : props.airplayAvailable === false
                      ? "No AirPlay devices found on this network"
                      : "Apple TV and AirPlay TVs"
                }
                checked={props.current === "airplay"}
                disabled={props.airplay !== false && props.airplayAvailable === false}
                onClick={() => pick(() => props.onPick({ kind: "airplay", id: "airplay", name: "AirPlay" }))}
              />
            </Show>
            <Row
              icon="cast"
              title="Chromecast or Google TV"
              detail={ccDetail()}
              checked={props.current === "chromecast"}
              disabled={!ccEnabled()}
              onClick={() => pick(() => props.onPick({ kind: "chromecast", id: "chromecast", name: "Chromecast" }))}
            />

            <Show when={props.dlna !== false && !dlnaMissing()}>
              <p class="sheet__section">TVs on your network</p>
              <Show when={!devices.loading || devices()} fallback={<p class="sheet__status" role="status"><span class="spinner" /> Looking for TVs…</p>}>
                <Show
                  when={!devices.error}
                  fallback={<p class="sheet__status" role="status">Couldn't look for TVs right now.</p>}
                >
                  <For
                    each={devices() ?? []}
                    fallback={<p class="sheet__status" role="status">No TVs found. Make sure the TV is on and connected to the same network.</p>}
                  >
                    {(d: CastDevice) => (
                      <Row
                        icon="tv"
                        title={d.name}
                        detail={props.current === d.id ? "Playing now" : [d.manufacturer, d.model].filter(Boolean).join(" ") || "TV"}
                        checked={props.current === d.id}
                        onClick={() => pick(() => props.onPick({ kind: "dlna", id: d.id, name: d.name, device: d }))}
                      />
                    )}
                  </For>
                </Show>
              </Show>
              <button
                type="button"
                class="sheet__link"
                disabled={refreshing()}
                onClick={async () => {
                  setRefreshing(true);
                  await refetch();
                  setRefreshing(false);
                }}
              >
                <Icon name="retry" /> {refreshing() ? "Searching…" : "Search again"}
              </button>
            </Show>
          </div>
          <footer class="sheet__foot">
            <Show when={props.footer} fallback={<><span class="keycap">Back</span> to close</>}>
              {props.footer}
            </Show>
          </footer>
        </div>
      </div>
    </Portal>
  );
}

function Row(props: { icon: IconName; title: string; detail: string; checked: boolean; disabled?: boolean; onClick: () => void }): JSX.Element {
  return (
    <button
      type="button"
      role="radio"
      class="sheet__opt sheet__opt--icon"
      aria-checked={props.checked ? "true" : "false"}
      disabled={props.disabled}
      onClick={() => props.onClick()}
    >
      <span class="sheet__mark">
        <Icon name={props.icon} />
      </span>
      <span class="sheet__text">
        <span class="sheet__title">
          {props.title}
          <Show when={props.checked}><small class="sheet__tag">Now</small></Show>
        </span>
        <span class="sheet__detail">{props.detail}</span>
      </span>
    </button>
  );
}

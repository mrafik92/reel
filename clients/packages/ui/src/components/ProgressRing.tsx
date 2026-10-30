import { Show, Switch, Match, type JSX } from "solid-js";
import type { RequestState } from "@reel/sdk";
import { Icon } from "./Icon";

export type RingState = RequestState | "idle";

type Mode = "pending" | "spin" | "determinate" | "done" | "error" | "muted";

const MODE: Record<RingState, Mode> = {
  idle: "muted",
  requested: "pending",
  approved: "pending",
  searching: "spin",
  queued: "spin",
  importing: "spin",
  downloading: "determinate",
  stalled: "determinate",
  available: "done",
  failed: "error",
  not_found: "error",
  declined: "muted",
};

export const RING_LABEL: Record<RingState, string> = {
  idle: "",
  requested: "Requested",
  approved: "Approved",
  searching: "Searching",
  queued: "Queued",
  downloading: "Downloading",
  stalled: "Stalled",
  importing: "Importing",
  available: "Ready",
  failed: "Failed",
  not_found: "Not found",
  declined: "Declined",
};

const R = 44;
const C = 2 * Math.PI * R;

export interface ProgressRingProps {
  state: RingState;
  /** 0..1, used for downloading/stalled. */
  progress?: number;
  /** Diameter in CSS units (default 4.5rem). */
  size?: string;
  /** Renders a retry button in the centre for failed/not_found. */
  onRetry?: () => void;
  /** Show the retry glyph without a button (the parent element handles the action). */
  retryable?: boolean;
  class?: string;
}

/**
 * Download progress ring (SPEC §7.1): indeterminate spinner for
 * searching/queued/importing, determinate % for downloading, amber when
 * stalled, red with retry for failed/not_found, check when available.
 */
export function ProgressRing(props: ProgressRingProps): JSX.Element {
  const mode = () => MODE[props.state] ?? "muted";
  const pct = () => Math.max(0, Math.min(1, props.progress ?? 0));
  const tone = () =>
    props.state === "stalled" ? "warn" : mode() === "error" ? "danger" : mode() === "muted" ? "muted" : "accent";
  const label = () => RING_LABEL[props.state];

  return (
    <div
      class={`ring ring--${mode()} ring--${tone()}${props.class ? " " + props.class : ""}`}
      style={{ "--ring-size": props.size ?? "4.5rem" } as JSX.CSSProperties}
      role={mode() === "determinate" ? "progressbar" : "status"}
      aria-valuemin={mode() === "determinate" ? 0 : undefined}
      aria-valuemax={mode() === "determinate" ? 100 : undefined}
      aria-valuenow={mode() === "determinate" ? Math.round(pct() * 100) : undefined}
      aria-label={`${label()}${mode() === "determinate" ? ` ${Math.round(pct() * 100)}%` : ""}`}
    >
      <svg class="ring__svg" viewBox="0 0 100 100" aria-hidden="true">
        <circle class="ring__track" cx="50" cy="50" r={R} />
        <Switch>
          <Match when={mode() === "determinate"}>
            <circle
              class="ring__arc"
              cx="50" cy="50" r={R}
              stroke-dasharray={`${C} ${C}`}
              stroke-dashoffset={C * (1 - pct())}
              transform="rotate(-90 50 50)"
            />
          </Match>
          <Match when={mode() === "spin"}>
            <g class="ring__spinner">
              <circle class="ring__arc" cx="50" cy="50" r={R} stroke-dasharray={`${C * 0.26} ${C}`} />
            </g>
          </Match>
          <Match when={mode() === "error"}>
            <circle class="ring__arc" cx="50" cy="50" r={R} />
          </Match>
          <Match when={mode() === "done"}>
            <circle class="ring__disc" cx="50" cy="50" r={R + 3.5} />
          </Match>
        </Switch>
      </svg>
      <div class="ring__center">
        <Switch>
          <Match when={mode() === "determinate"}>
            <span class="ring__pct">
              <Show when={props.state === "stalled"}>
                <Icon name="alert" class="ring__pct-icon" />
              </Show>
              {Math.floor(pct() * 100)}
              <small>%</small>
            </span>
          </Match>
          <Match when={mode() === "done"}>
            <Icon name="check" class="ring__icon ring__icon--pop" />
          </Match>
          <Match when={mode() === "error"}>
            <Show when={props.onRetry} fallback={<Icon name={props.retryable ? "retry" : "alert"} class="ring__icon" />}>
              <button
                type="button"
                class="ring__retry"
                aria-label="Retry"
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  props.onRetry?.();
                }}
              >
                <Icon name="retry" class="ring__icon" />
              </button>
            </Show>
          </Match>
          <Match when={props.state === "searching"}>
            <Icon name="search" class="ring__icon" />
          </Match>
          <Match when={props.state === "queued" || mode() === "pending"}>
            <Icon name="clock" class="ring__icon" />
          </Match>
          <Match when={props.state === "importing"}>
            <Icon name="layers" class="ring__icon" />
          </Match>
          <Match when={props.state === "declined"}>
            <Icon name="x" class="ring__icon" />
          </Match>
        </Switch>
      </div>
    </div>
  );
}

import { Show, type JSX } from "solid-js";
import type { DownloadProgress } from "../progress";
import { formatEta, formatSpeed } from "../format";
import { Img } from "./Img";
import { ProgressRing, RING_LABEL } from "./ProgressRing";
import { Icon } from "./Icon";

export type Availability = "available" | "partially" | "requested" | "none";

/** The subset of `DiscoverCardDto` the card needs. */
export interface DiscoverCardData {
  tmdb_id: number;
  media_type: "movie" | "tv";
  title: string;
  year?: number | null;
  poster_url?: string | null;
  status: Availability;
}

export interface DiscoverCardProps {
  card: DiscoverCardData;
  /** Live download state for this tmdb id (from the progress store). */
  download?: DownloadProgress;
  /** "Add" for admins/auto-approve, "Request" otherwise. */
  addLabel?: string;
  /** Request in flight. */
  busy?: boolean;
  onOpen: () => void;
  onAdd: () => void;
  /** Navigate to the library item (available titles). */
  onPlay: () => void;
  onRetry?: () => void;
  autofocus?: boolean;
  /** Replaces "year · Series" under the title (e.g. the character played). */
  subtitle?: string;
}

const ACTIVE = { searching: 1, queued: 1, downloading: 1, stalled: 1, importing: 1, approved: 1, requested: 1, failed: 1, not_found: 1 };

/**
 * Discover/TMDB card (SPEC §7.1): availability badge, one-click Add, and the
 * live progress ring that fills while the title downloads and flips to Play.
 */
export function DiscoverCard(props: DiscoverCardProps): JSX.Element {
  const c = () => props.card;
  const dl = () => props.download;
  const state = () => dl()?.state;
  const available = () => state() === "available" || (!dl() && c().status === "available");
  const ringOn = () => !!state() && (state()! in ACTIVE || state() === "available");
  const failed = () => state() === "failed" || state() === "not_found";
  const action = (): { kind: "play" | "retry" | "add" | "status"; label: string; icon: "play" | "retry" | "plus" | "clock" | "info" } => {
    if (available()) return { kind: "play", label: "Play", icon: "play" };
    if (failed() && props.onRetry) return { kind: "retry", label: "Retry", icon: "retry" };
    if (props.busy) return { kind: "add", label: "Adding…", icon: "clock" };
    if (ringOn()) {
      const d = dl()!;
      const pct = d.state === "downloading" || d.state === "stalled" ? ` ${Math.floor(d.progress * 100)}%` : "";
      return { kind: "status", label: RING_LABEL[d.state] + pct, icon: "info" };
    }
    if (c().status === "requested") return { kind: "status", label: "Requested", icon: "clock" };
    return { kind: "add", label: c().status === "partially" ? "Add more" : props.addLabel ?? "Add", icon: "plus" };
  };
  const label = () =>
    `${c().title}${c().year ? ", " + c().year : ""}${c().media_type === "tv" ? ", series" : ""}` +
    (state() ? `, ${RING_LABEL[state()!]}` : available() ? ", in your library" : c().status === "requested" ? ", requested" : "");

  return (
    <div class="dcard" classList={{ "has-download": ringOn(), "is-ready": available() }} data-fid={`t:${c().media_type}:${c().tmdb_id}`}>
      <button
        type="button"
        class="card card--poster dcard__poster"
        onClick={() => props.onOpen()}
        aria-label={label()}
        data-autofocus={props.autofocus ? "" : undefined}
      >
        <div class="card__frame">
          <Img
            class="card__img"
            src={c().poster_url}
            width={342}
            fallback={
              <div class="card__fallback" aria-hidden="true">
                <span>{c().title}</span>
              </div>
            }
          />
          <Show when={available()}>
            {/* Fresh arrivals: the badge lands as the ring's checkmark fades out. */}
            <span class="avail avail--available" classList={{ "avail--late": state() === "available" }}><Icon name="check" /> In library</span>
          </Show>
          <Show when={!ringOn()}>
            <Show when={c().status === "partially"}>
              <span class="avail avail--partially">Partly in library</span>
            </Show>
            <Show when={c().status === "requested"}>
              <span class="avail avail--requested"><Icon name="clock" /> Requested</span>
            </Show>
          </Show>
          <Show when={ringOn() && dl()}>
            {(d) => (
              <div class={`card__overlay card__overlay--${d().state}`}>
                <ProgressRing state={d().state} progress={d().progress} />
                <div class="card__dl-meta">
                  <strong>{d().state === "available" ? "Ready" : RING_LABEL[d().state]}</strong>
                  <Show when={d().state === "downloading" && (d().eta_seconds || d().speed_bps)}>
                    <span>
                      {d().eta_seconds ? formatEta(d().eta_seconds!) : ""}
                      {d().eta_seconds && d().speed_bps ? " · " : ""}
                      {d().speed_bps ? formatSpeed(d().speed_bps!) : ""}
                    </span>
                  </Show>
                </div>
              </div>
            )}
          </Show>
        </div>
        <div class="card__caption">
          <span class="card__title">{c().title}</span>
          <span class="card__sub">
            {props.subtitle ?? `${c().year ?? ""}${c().media_type === "tv" ? (c().year ? " · Series" : "Series") : ""}`}
          </span>
        </div>
      </button>
      <div class="dcard__action">
        {/* One persistent button whose role changes (Add → status → Play) so focus never drops on TV. */}
        <button
          type="button"
          class={`pill pill--${action().kind}`}
          aria-disabled={props.busy ? "true" : undefined}
          aria-label={`${action().label}: ${c().title}`}
          onClick={() => {
            // Never `disabled`: Chrome drops focus from disabled buttons, stranding the remote.
            if (props.busy) return;
            const k = action().kind;
            if (k === "play") props.onPlay();
            else if (k === "retry") props.onRetry?.();
            else if (k === "add") props.onAdd();
            else props.onOpen();
          }}
        >
          <Icon name={action().icon} />
          <span>{action().label}</span>
        </button>
      </div>
    </div>
  );
}

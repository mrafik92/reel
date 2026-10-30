import { Show, type JSX } from "solid-js";
import type { ItemSummary } from "../types";
import type { DownloadProgress } from "../progress";
import { episodeCode, formatEta, formatRuntime, formatSpeed } from "../format";
import { Img } from "./Img";
import { ProgressRing, RING_LABEL, type RingState } from "./ProgressRing";
import { Icon } from "./Icon";

export interface PosterCardProps {
  item: ItemSummary;
  href: string;
  /** poster = 2:3, landscape = 16:9 (episodes, continue watching). */
  variant?: "poster" | "landscape";
  /** Override the caption's second line. */
  subtitle?: string;
  /** Hide caption below the art (dense grids). */
  bare?: boolean;
  /** Live download state (SPEC §7.1); renders the ring overlay. */
  download?: DownloadProgress;
  onRetry?: () => void;
  /**
   * Background activity on an item that's already in the library (new episodes,
   * an upgrade): a small corner ring instead of the full overlay.
   */
  activity?: { state: RingState; progress: number };
  onFocus?: (item: ItemSummary) => void;
  onClick?: (e: MouseEvent) => void;
  /** Initial focus for the page. */
  autofocus?: boolean;
  /** Rating + runtime strip, revealed on focus/hover (default on when the item carries them). */
  meta?: boolean;
  class?: string;
}

const W = { poster: 342, landscape: 600 };

export function PosterCard(props: PosterCardProps): JSX.Element {
  const variant = () => props.variant ?? "poster";
  const it = () => props.item;
  const art = () =>
    variant() === "landscape" ? it().backdrop_url ?? it().poster_url : it().poster_url ?? it().backdrop_url;
  const sub = () => {
    if (props.subtitle != null) return props.subtitle;
    const i = it();
    if (i.kind === "episode") return episodeCode(i);
    return i.year ? String(i.year) : "";
  };
  const progress = () => {
    const p = it().progress;
    return p && p > 0.005 && p < 0.995 ? p : 0;
  };
  const dl = () => props.download;
  const dlActive = () => {
    const d = dl();
    return !!d && d.state !== "declined";
  };
  const rating = () => {
    const r = it().rating;
    return r && r > 0 ? r.toFixed(1) : "";
  };
  const runtime = () => (it().kind === "movie" ? formatRuntime(it().runtime_s) : "");
  const showMeta = () => props.meta !== false && it().kind !== "episode" && !dlActive() && !!(rating() || runtime());

  return (
    <a
      class={`card card--${variant()}${props.class ? " " + props.class : ""}`}
      classList={{ "has-download": dlActive(), "is-ready": dl()?.state === "available" }}
      href={props.href}
      data-fid={it().id}
      data-autofocus={props.autofocus ? "" : undefined}
      onFocus={() => props.onFocus?.(it())}
      onClick={(e) => {
        const st = dl()?.state;
        if (props.onRetry && (st === "failed" || st === "not_found")) {
          // The whole card is the retry target (no nested buttons inside the link).
          e.preventDefault();
          props.onRetry();
          return;
        }
        props.onClick?.(e);
      }}
      aria-label={`${it().title}${sub() ? ", " + sub() : ""}${showMeta() ? `${rating() ? ", rated " + rating() : ""}${runtime() ? ", " + runtime() : ""}` : ""}${dl() ? ", " + RING_LABEL[dl()!.state] : props.activity ? `, ${RING_LABEL[props.activity.state]} ${Math.round(props.activity.progress * 100)}%` : ""}`}
    >
      <div class="card__frame">
        <Img
          class="card__img"
          src={art()}
          width={W[variant()]}
          alt=""
          fallback={
            <div class="card__fallback" aria-hidden="true">
              <span>{it().title}</span>
            </div>
          }
        />
        <Show when={props.activity && !dlActive() && props.activity}>
          {(a) => (
            <span class={`card__activity card__activity--${a().state}`} title={`${RING_LABEL[a().state]} ${Math.round(a().progress * 100)}%`}>
              <ProgressRing state={a().state} progress={a().progress} size="2.4rem" />
            </span>
          )}
        </Show>
        <Show when={it().played && !dlActive() && !props.activity}>
          <span class="card__played" title="Watched">
            <Icon name="check" />
          </span>
        </Show>
        <Show when={showMeta()}>
          <span class="card__meta" classList={{ "has-progress": !!progress() }} aria-hidden="true">
            <Show when={rating()}>
              <span class="card__rating"><Icon name="star" filled /> {rating()}</span>
            </Show>
            <Show when={runtime()}>
              <span>{runtime()}</span>
            </Show>
          </span>
        </Show>
        <Show when={progress() && !dlActive()}>
          <div class="card__progress" aria-hidden="true">
            <i style={{ width: `${progress() * 100}%` }} />
          </div>
        </Show>
        <Show when={dlActive() && dl()}>
          {(d) => (
            <div class={`card__overlay card__overlay--${d().state}`}>
              <ProgressRing state={d().state} progress={d().progress} retryable={!!props.onRetry} />
              <div class="card__dl-meta">
                <strong>{d().state === "available" ? "Play now" : RING_LABEL[d().state]}</strong>
                <Show when={props.onRetry && (d().state === "failed" || d().state === "not_found")}>
                  <span>Select to retry</span>
                </Show>
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
        <Show when={dl()?.state === "available"}>
          <span class="card__play-chip">
            <Icon name="play" /> Play
          </span>
        </Show>
      </div>
      <Show when={!props.bare}>
        <div class="card__caption">
          <span class="card__title">{it().title}</span>
          <Show when={sub()}>
            <span class="card__sub">{sub()}</span>
          </Show>
        </div>
      </Show>
    </a>
  );
}

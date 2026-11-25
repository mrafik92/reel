import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import {
  Backdrop, focusElement, focusFirst, formatEta, formatRuntime, formatSpeed, Icon, Img, onBack, ProgressRing, RING_LABEL,
} from "@reel/ui";
import { api, type DiscoverCard, type MediaType } from "../api";
import { progressFor } from "../lib/downloads";
import { canAutoAdd } from "../lib/auth";
import { isBusy, useRequestActions } from "../lib/requests";
import { ErrorState } from "./States";
import { RatingsStrip, ReviewsSection, useReviews } from "./Reviews";
import { CastTile } from "./Cast";
import { vodTwin } from "./vod/links";

/** TMDB title detail (SPEC §7.1): availability, live ring, season picker, Add / Play / Retry. */
export function DiscoverSheet(props: { media: MediaType; tmdbId: number; seed?: DiscoverCard; onClose: () => void }): JSX.Element {
  const [d] = createResource(() => [props.media, props.tmdbId] as const, ([m, id]) => api.discoverDetail(m, id));
  const card = () => d()?.card ?? props.seed;
  const reviews = useReviews(() => ({ media: props.media, tmdbId: props.tmdbId }));
  const actions = useRequestActions();
  const dl = () => progressFor(props.tmdbId);
  const state = () => dl()?.state;
  const available = () => state() === "available" || (!dl() && card()?.status === "available");
  const active = () => !!state() && state() !== "available" && state() !== "declined";
  const failed = () => state() === "failed" || state() === "not_found";
  const seasons = () => d()?.seasons ?? [];
  const [picked, setPicked] = createSignal<number[] | null>(null); // null = all seasons
  const toggleSeason = (n: number) => {
    const all = seasons();
    const cur = picked() ?? all.slice();
    const next = cur.indexOf(n) >= 0 ? cur.filter((x) => x !== n) : cur.concat(n).sort((a, b) => a - b);
    setPicked(next.length === all.length ? null : next);
  };
  const isPicked = (n: number) => picked() == null || picked()!.indexOf(n) >= 0;

  let panel!: HTMLDivElement;
  let returnTo: HTMLElement | null = null;
  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => focusFirst(panel));
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });

  const add = async () => {
    const c = card();
    if (!c || (picked() != null && picked()!.length === 0)) return;
    const ok = await actions.add(c, props.media === "tv" ? picked() ?? undefined : undefined);
    if (ok) requestAnimationFrame(() => focusFirst(panel));
  };

  return (
    <Portal>
      <div class="sheet dsheet" data-nav-trap data-nav-fixed>
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel dsheet__panel" ref={panel} role="dialog" aria-modal="true" aria-label={card()?.title ?? "Title details"}>
          <div class="dsheet__art">
            <Backdrop src={card()?.backdrop_url} width={1280} class="dsheet__backdrop" />
            <button type="button" class="iconbtn dsheet__close" aria-label="Close" onClick={() => props.onClose()}>
              <Icon name="x" />
            </button>
          </div>
          <div class="dsheet__body" data-nav-scroll="y">
            <Show when={!d.error} fallback={<ErrorState error={d.error} />}>
              <Show when={card()}>
                {(c) => (
                  <>
                    <div class="dsheet__head">
                      <Img class="dsheet__poster" src={c().poster_url} width={342} fallback={<div class="card__fallback"><span>{c().title}</span></div>} />
                      <div>
                        <p class="eyebrow">{props.media === "tv" ? "Series" : "Film"}{c().year ? ` · ${c().year}` : ""}</p>
                        <h2 class="dsheet__title">{c().title}</h2>
                        <p class="meta">
                          <Show when={d()?.runtime_minutes}><span>{formatRuntime(d()!.runtime_minutes! * 60)}</span></Show>
                          <Show when={d()?.genres.length}><span>{d()!.genres.slice(0, 3).join(" / ")}</span></Show>
                          <Show when={c().vote_average}><span class="meta__rating"><Icon name="star" filled /> {c().vote_average!.toFixed(1)}</span></Show>
                        </p>
                        <RatingsStrip data={reviews()} />
                        <p class="dsheet__status">
                          <Show when={available()}><span class="avail avail--available avail--inline"><Icon name="check" /> In your library</span></Show>
                          <Show when={!available() && !active() && c().status === "partially"}><span class="avail avail--partially avail--inline">Partly in your library</span></Show>
                          <Show when={!available() && !active() && c().status === "requested"}><span class="avail avail--requested avail--inline"><Icon name="clock" /> Requested</span></Show>
                        </p>
                      </div>
                    </div>

                    <Show when={dl() && (active() || state() === "available")}>
                      <div class={`dsheet__progress dsheet__progress--${state()}`}>
                        <ProgressRing state={dl()!.state} progress={dl()!.progress} size="3.6rem" />
                        <div>
                          <strong>{state() === "available" ? "Ready to watch" : RING_LABEL[dl()!.state]}{state() === "downloading" ? ` · ${Math.floor(dl()!.progress * 100)}%` : ""}</strong>
                          <span>
                            {state() === "downloading"
                              ? [dl()!.eta_seconds ? formatEta(dl()!.eta_seconds!) : "", dl()!.speed_bps ? formatSpeed(dl()!.speed_bps!) : ""].filter(Boolean).join(" · ")
                              : state() === "searching" ? "Looking for a good release…"
                              : state() === "stalled" ? "No progress for a while — it’ll keep trying"
                              : state() === "importing" ? "Moving it into your library…"
                              : state() === "requested" ? "Waiting for an admin to approve"
                              : failed() ? "Nothing suitable found yet" : ""}
                          </span>
                        </div>
                      </div>
                    </Show>

                    <Show when={c().overview}>
                      <p class="dsheet__overview">{c().overview}</p>
                    </Show>

                    <Show when={props.media === "tv" && seasons().length && !available() && !active()}>
                      <div class="dsheet__seasons">
                        <p class="eyebrow">Seasons{picked() == null ? " · all" : ` · ${picked()!.length} selected`}</p>
                        <div class="chips" data-nav-group>
                          <For each={seasons()}>
                            {(n) => (
                              <button type="button" class="chip chip--toggle" aria-pressed={isPicked(n)} onClick={() => toggleSeason(n)}>
                                <Show when={isPicked(n)}><Icon name="check" /></Show> Season {n}
                              </button>
                            )}
                          </For>
                        </div>
                      </div>
                    </Show>

                    <div class="actions">
                      <Show when={available()}>
                        <button type="button" class="btn btn--primary btn--lg" data-autofocus onClick={() => void actions.play({ ...c(), media_type: props.media })}>
                          <Icon name="play" /> <span>Play</span>
                        </button>
                      </Show>
                      <Show when={failed() && dl()?.request_id}>
                        <button type="button" class="btn btn--accent btn--lg" data-autofocus onClick={() => void actions.retry(dl()!.request_id, c().title)}>
                          <Icon name="retry" /> <span>Try again</span>
                        </button>
                      </Show>
                      <Show when={!available() && !active() && c().status !== "requested"}>
                        <button
                          type="button"
                          class="btn btn--accent btn--lg"
                          data-autofocus
                          aria-disabled={isBusy(props.media, props.tmdbId) || (picked() != null && picked()!.length === 0) ? "true" : undefined}
                          onClick={() => void add()}
                        >
                          <Icon name="plus" />
                          <span>
                            {isBusy(props.media, props.tmdbId) ? "Adding…" : `${canAutoAdd() ? "Add" : "Request"}${c().status === "partially" ? " missing" : ""}`}
                            {props.media === "tv" && picked() != null ? ` ${picked()!.length} season${picked()!.length === 1 ? "" : "s"}` : ""}
                          </span>
                        </button>
                      </Show>
                      <Show when={!available() && vodTwin(c())}>
                        {(h) => (
                          <a class="btn btn--iptv" href={h()} onClick={() => props.onClose()}>
                            <Icon name="live" /> <span>Play now (IPTV)</span>
                          </a>
                        )}
                      </Show>
                      <button type="button" class="btn" onClick={() => props.onClose()}>
                        <span>Close</span>
                      </button>
                    </div>
                    <Show when={!canAutoAdd() && !available() && !active()}>
                      <p class="dsheet__fine">Requests are reviewed by your server’s admin before downloading.</p>
                    </Show>
                    <Show when={d()?.cast?.length}>
                      <div class="dsheet__cast">
                        <p class="eyebrow">Cast</p>
                        <div class="row__scroller dsheet__cast-scroll" data-nav-scroll="x" data-nav-group>
                          <div class="row__track">
                            <For each={d()!.cast!.slice(0, 15)}>{(c) => <CastTile c={c} />}</For>
                          </div>
                        </div>
                      </div>
                    </Show>

                    <ReviewsSection data={reviews()} compact limit={3} />
                  </>
                )}
              </Show>
            </Show>
          </div>
        </div>
      </div>
    </Portal>
  );
}

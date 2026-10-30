import { createEffect, createResource, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import {
  Backdrop, focusElement, focusFirst, formatRuntime, Icon, Img, onBack,
  type ItemDetail, type ItemSummary, type LibraryKind,
} from "@reel/ui";
import { api, ApiError, type GenreCount, type LibraryFilters } from "../api";
import { itemHref, playHref } from "../lib/links";
import { activeCount, DECADES, describePool, RATINGS, RUNTIMES } from "../lib/browse";

/** Glyphs not in the shared icon set (kept local to the picker). */
export function Glyph(props: { name: "dice" | "sliders" | "shuffle"; class?: string }): JSX.Element {
  const d = {
    dice: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z",
    sliders: "M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1M15 4v4M9 10v4M17 16v4",
    shuffle: "M16 4h4v4M4 20 20 4M20 16v4h-4M15 15l5 5M4 4l5 5",
  }[props.name];
  return (
    <svg class={"icon" + (props.class ? " " + props.class : "")} viewBox="0 0 24 24" width="1em" height="1em" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d={d} />
      {props.name === "dice" && (
        <g fill="currentColor" stroke="none">
          <circle cx="8.2" cy="8.2" r="1.5" />
          <circle cx="15.8" cy="8.2" r="1.5" />
          <circle cx="12" cy="12" r="1.5" />
          <circle cx="8.2" cy="15.8" r="1.5" />
          <circle cx="15.8" cy="15.8" r="1.5" />
        </g>
      )}
    </svg>
  );
}

// ---------------------------------------------------------------------------
// Genre chip bar

export function GenreBar(props: {
  genres: GenreCount[];
  active: string[];
  allLabel: string;
  /** Lanes on screen: no chip is "selected" yet. */
  browsing?: boolean;
  onPick: (genre: string | null) => void;
}): JSX.Element {
  const isOn = (g: string) => props.active.length === 1 && props.active[0] === g;
  return (
    <div class="genre-bar" data-nav-group>
      <div class="genre-bar__scroller" data-nav-scroll="x" role="toolbar" aria-label="Genres">
        <div class="genre-bar__track">
          <button
            type="button"
            class="gchip gchip--all"
            aria-pressed={!props.browsing && props.active.length === 0}
            data-nav-default={props.active.length === 0 ? "" : undefined}
            onClick={() => props.onPick(null)}
          >
            {props.allLabel}
          </button>
          <For each={props.genres}>
            {(g) => (
              <button
                type="button"
                class="gchip"
                aria-pressed={isOn(g.name)}
                data-nav-default={isOn(g.name) ? "" : undefined}
                onClick={() => props.onPick(isOn(g.name) ? null : g.name)}
              >
                {g.name}
                <span class="gchip__count">{g.count}</span>
              </button>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Filter panel (side sheet)

function useModal(panel: () => HTMLElement, onClose: () => void, focus?: () => HTMLElement | null) {
  let returnTo: HTMLElement | null = null;
  onMount(() => {
    returnTo = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => {
      const f = focus?.();
      if (f) focusElement(f, { smooth: false });
      else focusFirst(panel());
    });
    const off = onBack(() => {
      onClose();
      return true;
    });
    onCleanup(() => {
      off();
      if (returnTo && returnTo.isConnected) focusElement(returnTo, { smooth: false });
    });
  });
}

export function FilterPanel(props: {
  kind: LibraryKind;
  filters: LibraryFilters;
  genres: GenreCount[];
  onChange: (patch: Partial<LibraryFilters>) => void;
  onClear: () => void;
  onClose: () => void;
}): JSX.Element {
  let panel!: HTMLDivElement;
  useModal(() => panel, () => props.onClose());
  const f = () => props.filters;
  const genreOn = (g: string) => (f().genre ?? []).indexOf(g) >= 0;
  const toggleGenre = (g: string) => {
    const cur = f().genre ?? [];
    props.onChange({ genre: genreOn(g) ? cur.filter((x) => x !== g) : cur.concat(g) });
  };
  const decadeOn = (d: { from?: number; to?: number }) => f().year_from === d.from && f().year_to === d.to;
  const anyDecade = () => f().year_from == null && f().year_to == null;
  const n = () => activeCount(f());

  return (
    <Portal>
      <div class="sheet fpanel" data-nav-trap data-nav-fixed>
        <div class="sheet__scrim" onClick={() => props.onClose()} />
        <div class="sheet__panel fpanel__panel" ref={panel} role="dialog" aria-modal="true" aria-label="Filters">
          <header class="fpanel__head">
            <p class="eyebrow eyebrow--accent">Narrow it down</p>
            <h2>Filters</h2>
          </header>
          <div class="fpanel__body" data-nav-scroll="y">
            <Show when={props.kind === "movies"}>
              <section class="fpanel__sec">
                <h3 class="fpanel__label">Tonight I have</h3>
                <div class="segmented" role="group" aria-label="Tonight I have" data-nav-group>
                  <For each={RUNTIMES}>
                    {(r) => (
                      <button type="button" class="segmented__btn" aria-pressed={f().max_runtime_min === r.value} onClick={() => props.onChange({ max_runtime_min: r.value })}>
                        {r.label}
                      </button>
                    )}
                  </For>
                </div>
              </section>
            </Show>

            <section class="fpanel__sec fpanel__sec--inline">
              <div>
                <h3 class="fpanel__label">Only things I haven't seen</h3>
                <p class="fpanel__hint">Hides anything you've finished.</p>
              </div>
              <button
                type="button"
                class="switch"
                role="switch"
                aria-checked={f().unwatched ? "true" : "false"}
                aria-label="Unwatched only"
                onClick={() => props.onChange({ unwatched: !f().unwatched })}
              >
                <i />
              </button>
            </section>

            <section class="fpanel__sec">
              <h3 class="fpanel__label">Rating</h3>
              <div class="segmented" role="group" aria-label="Minimum rating" data-nav-group>
                <For each={RATINGS}>
                  {(r) => (
                    <button type="button" class="segmented__btn" aria-pressed={f().min_rating === r.value} onClick={() => props.onChange({ min_rating: r.value })}>
                      <Show when={r.value}><Icon name="star" filled class="segmented__star" /></Show>
                      {r.label}
                    </button>
                  )}
                </For>
              </div>
            </section>

            <section class="fpanel__sec">
              <h3 class="fpanel__label">Decade</h3>
              <div class="chips" data-nav-group>
                <button type="button" class="chip" aria-pressed={anyDecade()} onClick={() => props.onChange({ year_from: undefined, year_to: undefined })}>
                  Any
                </button>
                <For each={DECADES}>
                  {(d) => (
                    <button type="button" class="chip" aria-pressed={decadeOn(d)} onClick={() => props.onChange(decadeOn(d) ? { year_from: undefined, year_to: undefined } : { year_from: d.from, year_to: d.to })}>
                      {d.label}
                    </button>
                  )}
                </For>
              </div>
            </section>

            <section class="fpanel__sec">
              <h3 class="fpanel__label">
                Genres <Show when={(f().genre?.length ?? 0) > 1}><span class="fpanel__hint fpanel__hint--inline">matching all {f().genre!.length}</span></Show>
              </h3>
              <div class="chips" data-nav-group>
                <For each={props.genres}>
                  {(g) => (
                    <button type="button" class="chip" aria-pressed={genreOn(g.name)} onClick={() => toggleGenre(g.name)}>
                      <Show when={genreOn(g.name)}><Icon name="check" class="chip__check" /></Show>
                      {g.name}
                    </button>
                  )}
                </For>
              </div>
            </section>
          </div>
          <footer class="fpanel__foot">
            <button type="button" class="btn btn--primary" onClick={() => props.onClose()}>
              <Icon name="check" /> <span>Show results</span>
            </button>
            <button type="button" class="btn btn--ghost" disabled={!n()} onClick={() => props.onClear()}>
              <Icon name="x" /> <span>Clear all{n() ? ` (${n()})` : ""}</span>
            </button>
          </footer>
        </div>
      </div>
    </Portal>
  );
}

// ---------------------------------------------------------------------------
// Surprise me — spotlight

export function Spotlight(props: {
  libraryId: number;
  kind: LibraryKind;
  filters: LibraryFilters;
  onClose: () => void;
  onClearFilters?: () => void;
}): JSX.Element {
  let root!: HTMLDivElement;
  let playBtn: HTMLAnchorElement | undefined;
  const noun = props.kind === "shows" ? "series" : "films";
  // Picking is for tonight: default to things you haven't seen.
  const [unwatched, setUnwatched] = createSignal(true);
  const pool = (): LibraryFilters => ({ ...props.filters, unwatched: unwatched() || props.filters.unwatched });
  const seen: number[] = [];
  const [pick, setPick] = createSignal<ItemSummary>();
  const [busy, setBusy] = createSignal(false);
  const [none, setNone] = createSignal(false);
  const [error, setError] = createSignal<unknown>();
  let token = 0;

  const roll = async () => {
    const my = ++token;
    setBusy(true);
    setError(undefined);
    try {
      let it: ItemSummary | undefined;
      // Random picks can repeat; reroll a few times to keep it feeling fresh.
      for (let i = 0; i < 4; i++) {
        it = await api.pick(props.libraryId, pool());
        if (seen.indexOf(it.id) < 0) break;
      }
      if (my !== token) return;
      seen.push(it!.id);
      if (seen.length > 12) seen.shift();
      setNone(false);
      setPick(it);
    } catch (e) {
      if (my !== token) return;
      if (e instanceof ApiError && e.status === 404) {
        setNone(true);
        setPick(undefined);
      } else setError(e);
    } finally {
      if (my === token) setBusy(false);
    }
  };
  createEffect(on(unwatched, () => void roll()));

  const [detail] = createResource(() => pick()?.id, (id) => api.item(id).catch(() => undefined as ItemDetail | undefined));
  const d = () => (detail() && detail()!.id === pick()?.id ? detail() : undefined);
  const rating = () => pick()?.rating ?? d()?.rating;
  const runtime = () => pick()?.runtime_s ?? d()?.runtime_s;
  const genres = () => (pick()?.genres && pick()!.genres!.length ? pick()!.genres! : d()?.genres ?? []);

  useModal(() => root, () => props.onClose(), () => playBtn ?? root.querySelector<HTMLElement>(".spot__again"));
  // First result: land on Play.
  createEffect(on(pick, (p, prev) => {
    if (p && !prev && playBtn) requestAnimationFrame(() => playBtn && focusElement(playBtn, { smooth: false }));
  }));

  return (
    <Portal>
      <div class="spot" classList={{ "is-busy": busy() }} ref={root} data-nav-trap data-nav-fixed role="dialog" aria-modal="true" aria-label="Surprise pick">
        <Backdrop src={pick()?.backdrop_url} width={1920} class="spot__backdrop" />
        <div class="spot__scrim" aria-hidden="true" />
        <button type="button" class="iconbtn spot__close" aria-label="Close" onClick={() => props.onClose()}>
          <Icon name="x" />
        </button>

        <div class="spot__stage">
          <div class="spot__copy">
            <p class="eyebrow eyebrow--accent spot__eyebrow">
              <Glyph name="dice" class="spot__dice" /> Tonight's pick
            </p>
            <p class="spot__pool">From your {describePool(pool(), noun)}</p>

            <Show when={!none() && !error()} fallback={
              <div class="spot__none">
                <h2 class="spot__title spot__title--none">{error() ? "Couldn't pick right now" : "Nothing fits all of that"}</h2>
                <p class="spot__overview">
                  {error() ? (error() instanceof Error ? (error() as Error).message : "Try again in a moment.") : activeCount(props.filters) === 0 ? "You've seen everything here — include watched titles to pick from the lot." : unwatched() ? "No unwatched titles match those filters. Include watched ones, or loosen the filters." : "Try fewer genres, a lower rating or a longer runtime."}
                </p>
              </div>
            }>
              <Show when={pick()} keyed fallback={<div class="spot__ghost" aria-hidden="true"><span class="skel skel--title" /><span class="skel skel--text" style={{ width: "18rem" }} /></div>}>
                {(p) => (
                  <div class="spot__reveal">
                    <h2 class="spot__title">{p.title}</h2>
                    <p class="meta spot__meta">
                      <Show when={p.year}><span>{p.year}</span></Show>
                      <Show when={rating()}><span class="meta__rating"><Icon name="star" filled /> {rating()!.toFixed(1)}</span></Show>
                      <Show when={runtime() && props.kind === "movies"}><span>{formatRuntime(runtime())}</span></Show>
                      <Show when={d()?.content_rating}><span class="badge">{d()!.content_rating}</span></Show>
                      <Show when={genres().length}><span>{genres().slice(0, 3).join(" / ")}</span></Show>
                    </p>
                    <p class="spot__overview">{d()?.overview ?? "\u00a0"}</p>
                  </div>
                )}
              </Show>
            </Show>

            <div class="actions spot__actions" data-nav-group>
              <Show when={pick() && !none()}>
                <a
                  ref={playBtn}
                  class="btn btn--primary btn--lg"
                  href={props.kind === "movies" ? playHref(pick()!) : itemHref(pick()!)}
                  data-autofocus
                >
                  <Icon name="play" /> <span>{props.kind === "movies" ? (pick()!.progress ? "Resume" : "Play") : "Start watching"}</span>
                </a>
              </Show>
              <button type="button" class="btn btn--lg spot__again" onClick={() => void roll()} aria-busy={busy()}>
                <Glyph name="shuffle" class="spot__shuffle" /> <span>{none() || error() ? "Try again" : "Another one"}</span>
              </button>
              <Show when={pick() && !none()}>
                <a class="btn btn--lg" href={itemHref(pick()!)}>
                  <Icon name="info" /> <span>Details</span>
                </a>
              </Show>
              <Show when={none() && activeCount(props.filters) > 0 && props.onClearFilters}>
                <button type="button" class="btn btn--lg" onClick={() => { props.onClearFilters!(); setTimeout(() => void roll(), 0); }}>
                  <Icon name="x" /> <span>Clear filters</span>
                </button>
              </Show>
            </div>
            <button
              type="button"
              class="chip spot__toggle"
              role="switch"
              aria-checked={unwatched() ? "true" : "false"}
              disabled={!!props.filters.unwatched}
              onClick={() => setUnwatched(!unwatched())}
            >
              <span class="spot__tick" aria-hidden="true">{unwatched() ? <Icon name="check" /> : null}</span>
              Only ones I haven't seen
            </button>
          </div>

          <Show when={!none() ? pick() : undefined} keyed>
            {(p) => (
              <div class="spot__poster" aria-hidden="true">
                <Img src={p.poster_url} width={500} class="spot__poster-img" fallback={<div class="card__fallback"><span>{p.title}</span></div>} />
              </div>
            )}
          </Show>
        </div>
      </div>
    </Portal>
  );
}

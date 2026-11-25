import { createEffect, createMemo, createResource, createSignal, For, on, Show, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import {
  Icon, PosterCard, Row, Sheet, VirtualGrid, isTvMode,
  type ItemSummary, type Library as Lib, type LibraryKind, type LibrarySort,
} from "@reel/ui";
import { api, ApiError, type BrowseLane, type LibraryFilters } from "../api";
import { usePageFocus } from "../lib/focus";
import { itemActivity } from "../lib/downloads";
import { isAdmin } from "../lib/auth";
import { itemHref, libraryHref } from "../lib/links";
import { activeCount, filterChips, filtersFrom, laneHref, paramsFrom, RUNTIMES, type BrowseParams } from "../lib/browse";
import { EmptyState, ErrorState, FirstRun, SkeletonRow } from "../components/States";
import { FilterPanel, GenreBar, Glyph, Spotlight } from "../components/Picker";
import { VodBrowser } from "../components/vod/VodBrowser";
import { cardPreview } from "../lib/preview";
import { isPhoneWidth } from "../lib/viewport";
import { rememberSource, SourceSwitch, storedSource, useVodCount, type Source } from "../components/vod/SourceSwitch";

const SORTS: { value: LibrarySort; label: string; detail?: string }[] = [
  { value: "title", label: "A–Z" },
  { value: "added", label: "Recently added" },
  { value: "year", label: "Newest first", detail: "By release year" },
  { value: "rating", label: "Highest rated" },
  { value: "runtime", label: "Shortest first", detail: "By runtime" },
  { value: "random", label: "Shuffle", detail: "A fresh random order" },
];
const PAGE = 60;

interface GridState {
  items: ItemSummary[];
  cursor: string | null | undefined;
  done: boolean;
}
/** Survives navigation so Back restores the grid instantly. */
const gridCache = new Map<string, GridState>();

export default function Library(props: { kind: LibraryKind }): JSX.Element {
  const [params, setParams] = useSearchParams<BrowseParams>();
  const [libs, { refetch }] = createResource(() => api.libraries());
  const ofKind = () => (libs() ?? []).filter((l) => l.kind === props.kind);
  const lib = (): Lib | undefined => ofKind().find((l) => String(l.id) === params.lib) ?? ofKind()[0];
  const sort = (): LibrarySort => (SORTS.some((s) => s.value === params.sort) ? (params.sort as LibrarySort) : "title");
  const seed = () => Number(params.seed) || 1;
  const filters = createMemo<LibraryFilters>(() => filtersFrom(params), filtersFrom({}), {
    equals: (a, b) => JSON.stringify(a) === JSON.stringify(b),
  });
  const nFilters = () => activeCount(filters());
  const movies = props.kind === "movies";
  const noun = movies ? "films" : "series";

  // ---- Source: My library · IPTV (remembered per page) ----------------------
  const vodCount = useVodCount(props.kind);
  const vodOn = () => (vodCount() ?? 0) > 0;
  const wanted = (): Source => {
    const p = params.source;
    if (p === "iptv" || p === "library") return p;
    return storedSource(props.kind) ?? (libs() && ofKind().length === 0 ? "iptv" : "library");
  };
  /** Waiting to learn whether IPTV exists before showing a remembered IPTV choice. */
  const pending = () => wanted() === "iptv" && vodCount.loading;
  const iptv = () => wanted() === "iptv" && vodOn();
  const setSource = (s: Source) => {
    rememberSource(props.kind, s);
    setParams(
      { ...paramsFrom({}), source: s, lib: undefined, view: undefined, sort: undefined, seed: undefined, cat: undefined, q: undefined },
      { replace: true },
    );
  };
  const libCount = () => (libs() ? ofKind().reduce((a, l) => a + l.item_count, 0) : undefined);
  const [vodReady, setVodReady] = createSignal(false);
  const switcher = () => (
    <Show when={vodOn()}>
      <SourceSwitch value={iptv() ? "iptv" : "library"} libraryCount={libCount()} iptvCount={vodCount()} onChange={setSource} />
    </Show>
  );

  const [genres] = createResource(() => lib()?.id, (id) => api.genres(id).catch(() => []));
  // Older servers have no lanes: fall back to the plain grid.
  const [lanes] = createResource(() => lib()?.id, (id) => api.browseLanes(id).catch(() => null as BrowseLane[] | null));
  const lanesOff = () => !lanes.loading && (!lanes() || lanes()!.filter((l) => l.items.length).length === 0);
  const grid = () => params.view === "all" || nFilters() > 0 || !!params.sort || lanesOff();
  // Keep Browse light on TV hardware: the chip bar covers every other genre.
  const visibleLanes = createMemo(() => {
    let genresLeft = 6;
    return (lanes() ?? []).filter((l) => l.items.length && (l.kind !== "genre" || genresLeft-- > 0));
  });

  // ---- Grid paging -------------------------------------------------------
  const [items, setItems] = createSignal<ItemSummary[]>([]);
  const [error, setError] = createSignal<unknown>();
  const [ready, setReady] = createSignal(false);
  const [done, setDone] = createSignal(false);
  let state: GridState = { items: [], cursor: undefined, done: false };
  let loading = false;
  let key = "";

  const loadMore = async () => {
    const l = lib();
    if (!l || loading || state.done) return;
    loading = true;
    const myKey = key;
    try {
      const page = await api.libraryItems(l.id, { sort: sort(), seed: seed(), cursor: state.cursor, limit: PAGE, filters: filters() });
      if (myKey !== key) return;
      state.items = state.items.concat(page.items);
      state.cursor = page.next_cursor;
      state.done = !page.next_cursor;
      setItems(state.items);
      setDone(state.done);
      setReady(true);
    } catch (e) {
      if (myKey === key) setError(e);
    } finally {
      if (myKey === key) loading = false;
    }
  };

  createEffect(
    on([lib, sort, seed, filters, grid], ([l, s, sd, f, g]) => {
      if (!l || !g) return;
      const k = `${l.id}:${s}:${s === "random" ? sd : ""}:${JSON.stringify(f)}`;
      if (k === key) return;
      key = k;
      setError(undefined);
      const hit = gridCache.get(key);
      state = hit ?? { items: [], cursor: undefined, done: false };
      gridCache.set(key, state);
      setItems(state.items);
      setDone(state.done);
      loading = false;
      if (state.items.length || state.done) setReady(true);
      else void loadMore();
    }),
  );
  const onRange = (_a: number, end: number) => {
    if (end + PAGE / 2 > state.items.length) void loadMore();
  };
  const total = () => {
    const l = lib();
    if (!l) return 0;
    if (nFilters() === 0) return Math.max(l.item_count, items().length);
    return done() ? items().length : items().length + 12;
  };

  // ---- URL helpers ---------------------------------------------------------
  const setFilters = (patch: Partial<LibraryFilters>) =>
    setParams({ ...paramsFrom({ ...filters(), ...patch }) }, { replace: true });
  const clearFilters = () => setParams({ ...paramsFrom({}) }, { replace: true });
  const setView = (all: boolean) =>
    setParams(all ? { view: "all" } : { ...paramsFrom({}), view: undefined, sort: undefined, seed: undefined });
  const setSort = (s: LibrarySort) =>
    setParams({ sort: s === "title" ? undefined : s, seed: s === "random" ? String(Math.floor(Math.random() * 1e6) + 1) : undefined, view: "all" }, { replace: true });

  const [spot, setSpot] = createSignal(false);
  const [panel, setPanel] = createSignal(false);
  const [sortSheet, setSortSheet] = createSignal(false);

  let el!: HTMLDivElement;
  usePageFocus(() => (pending() ? false : iptv() ? vodReady() : (grid() ? ready() : !lanes.loading) || (!!libs() && !lib())), () => el);

  const tv = isTvMode();
  const sortLabel = () => SORTS.find((s) => s.value === sort())!.label;

  return (
    <div class="page library" classList={{ "library--grid": !iptv() && grid(), "library--browse": !iptv() && !grid(), "library--iptv": iptv() }} ref={el}>
      <Show when={!pending()} fallback={<div class="library__lanes library__pending"><SkeletonRow /><SkeletonRow /></div>}>
      <Show when={!iptv()} fallback={<VodBrowser kind={movies ? "movie" : "series"} switcher={switcher()} onReady={() => setVodReady(true)} />}>
      <Show when={!libs.error} fallback={<ErrorState error={libs.error} onRetry={refetch} />}>
        <header class="page-head picker">
          <div class="page-head__row picker__title-row" data-nav-group>
            <h1 class="page-title">
              {lib()?.name ?? (movies ? "Movies" : "Shows")}
              <Show when={lib()}>
                <span class="page-title__count">{lib()!.item_count.toLocaleString()}</span>
              </Show>
            </h1>
            {switcher()}
            <span class="picker__spacer" />
            <Show when={lib() && lib()!.item_count > 0}>
              <button type="button" class="btn picker__surprise" onClick={() => setSpot(true)} data-autofocus={!grid() ? "" : undefined}>
                <Glyph name="dice" /> <span>Surprise me</span>
              </button>
              <button type="button" class="btn picker__filters" onClick={() => setPanel(true)}>
                <Glyph name="sliders" /> <span>Filters</span>
                <Show when={nFilters()}><span class="picker__badge">{nFilters()}</span></Show>
              </button>
              <Show when={grid()}>
                <button type="button" class="btn btn--dropdown picker__sort" aria-label={`Sort: ${sortLabel()}`} onClick={() => setSortSheet(true)}>
                  <span>{sortLabel()}</span> <Icon name="down" />
                </button>
                <Show when={sort() === "random"}>
                  <button type="button" class="iconbtn" aria-label="Shuffle again" title="Shuffle again" onClick={() => setSort("random")}>
                    <Glyph name="shuffle" />
                  </button>
                </Show>
              </Show>
            </Show>
          </div>
          <Show when={ofKind().length > 1}>
            <nav class="tabs picker__libs" aria-label="Libraries" data-nav-group>
              <For each={ofKind()}>
                {(l) => (
                  <a class="tab" href={libraryHref(l)} aria-current={l.id === lib()?.id ? "page" : undefined}>
                    {l.name}
                  </a>
                )}
              </For>
            </nav>
          </Show>

          <Show when={lib() && lib()!.item_count > 0}>
            <div class="picker__bar" data-nav-group>
              <Show when={!lanesOff()}>
                <div class="segmented" role="group" aria-label="View">
                  <button type="button" class="segmented__btn" aria-pressed={!grid()} onClick={() => setView(false)}>
                    Browse
                  </button>
                  <button type="button" class="segmented__btn" aria-pressed={grid()} onClick={() => setView(true)}>
                    <Icon name="grid" class="segmented__icon" /> All {noun}
                  </button>
                </div>
              </Show>
              <Show when={movies}>
                <div class="picker__time">
                  <span class="picker__label" id="tonight">Tonight I have</span>
                  <div class="segmented" role="group" aria-labelledby="tonight">
                    <For each={RUNTIMES}>
                      {(r) => (
                        <button type="button" class="segmented__btn" aria-pressed={filters().max_runtime_min === r.value} onClick={() => setFilters({ max_runtime_min: r.value })}>
                          {r.value ? r.label : "Any"}
                        </button>
                      )}
                    </For>
                  </div>
                </div>
              </Show>
              <button type="button" class="chip chip--toggle" role="switch" aria-checked={filters().unwatched ? "true" : "false"} onClick={() => setFilters({ unwatched: !filters().unwatched })}>
                <span class="chip__box" aria-hidden="true">{filters().unwatched ? <Icon name="check" /> : null}</span>
                Unwatched
              </button>
            </div>

            <Show when={(genres() ?? []).length}>
              <GenreBar
                genres={genres()!}
                active={filters().genre ?? []}
                allLabel="All genres"
                browsing={!grid()}
                onPick={(g) => setFilters({ genre: g ? [g] : undefined })}
              />
            </Show>

            <Show when={grid() && nFilters()}>
              <div class="picker__active" data-nav-group>
                <span class="picker__found" aria-live="polite">
                  {done() ? `${items().length.toLocaleString()} ${items().length === 1 ? (movies ? "film" : "series") : noun}` : "Filtered"}
                </span>
                <For each={filterChips(filters())}>
                  {(c) => (
                    <button type="button" class="fchip" onClick={() => setFilters(c.clear)} aria-label={`Remove ${c.label}`}>
                      {c.label} <Icon name="x" />
                    </button>
                  )}
                </For>
                <button type="button" class="fchip fchip--clear" onClick={clearFilters}>Clear all</button>
              </div>
            </Show>
          </Show>
        </header>

        <Show when={libs() && !lib()}>
          <FirstRun />
        </Show>

        {/* ---- Browse: curated lanes ---- */}
        <Show when={lib() && !grid()}>
          <Show when={!lanes.loading} fallback={<div class="library__lanes"><SkeletonRow /><SkeletonRow /></div>}>
            <div class="library__lanes">
              <For each={visibleLanes()}>
                {(lane, i) => (
                  <Row
                    title={lane.title}
                    subtitle={lane.subtitle}
                    variant={lane.kind === "top_unwatched" ? "ranked" : "poster"}
                    class={`row--lane row--lane-${lane.kind}`}
                    action={laneHref(lane, libs() ?? []) ? { label: "Explore all", href: laneHref(lane, libs() ?? [])! } : undefined}
                  >
                    <For each={lane.kind === "top_unwatched" ? lane.items.slice(0, 10) : lane.items}>
                      {(it, n) => (
                        <PosterCard
                          item={it}
                          href={itemHref(it)}
                          rank={lane.kind === "top_unwatched" ? n() + 1 : undefined}
                          activity={itemActivity(it.id)}
                          preview={cardPreview(itemHref(it))}
                        />
                      )}
                    </For>
                  </Row>
                )}
              </For>
              <div class="lane-cta">
                <p>
                  <strong>Know what you want?</strong> Every one of your {lib()!.item_count.toLocaleString()} {noun}, A–Z.
                </p>
                <button type="button" class="btn" onClick={() => setView(true)}>
                  <Icon name="grid" /> <span>All {noun}</span>
                </button>
              </div>
            </div>
          </Show>
        </Show>

        {/* ---- Grid: everything / filtered ---- */}
        <Show when={lib() && grid()}>
          <div class="library__body">
            <Show when={error()}>
              <Show
                when={error() instanceof ApiError && (error() as ApiError).status === 400}
                fallback={<ErrorState error={error()} onRetry={() => { setError(undefined); void loadMore(); }} />}
              >
                <EmptyState title="Those filters don’t add up" body={(error() as ApiError).message}>
                  <button type="button" class="btn btn--primary" onClick={() => { clearFilters(); setParams({ sort: undefined, seed: undefined }, { replace: true }); }} data-autofocus>
                    <Icon name="x" /> <span>Clear filters</span>
                  </button>
                </EmptyState>
              </Show>
            </Show>
            <Show when={!error()}>
              <Show
                when={lib()!.item_count > 0 || items().length > 0}
                fallback={<EmptyState title="This library is empty" body={isAdmin() ? "Run a scan from Settings → Libraries to pick up new files." : "New titles will show up here once the server finds them."} />}
              >
                <Show
                  when={!(done() && items().length === 0)}
                  fallback={
                    <EmptyState title="Nothing matches all of that" body="Try fewer genres, a wider decade or a longer runtime.">
                      <button type="button" class="btn btn--primary" onClick={clearFilters} data-autofocus>
                        <Icon name="x" /> <span>Clear filters</span>
                      </button>
                    </EmptyState>
                  }
                >
                  <VirtualGrid
                    class="library__grid"
                    items={items()}
                    total={total()}
                    minColumnRem={tv ? 9.6 : isPhoneWidth() ? 6.4 : 10.5}
                    ratio={1.5}
                    captionRem={isPhoneWidth() ? 3.1 : 0}
                    gapRem={tv ? 0.7 : 0.5}
                    onRange={onRange}
                    renderItem={(it) => <PosterCard item={it} href={itemHref(it)} activity={itemActivity(it.id)} preview={cardPreview(itemHref(it))} />}
                    renderPlaceholder={() => (
                      <div class="card card--poster is-placeholder">
                        <div class="card__frame skel" />
                      </div>
                    )}
                  />
                </Show>
              </Show>
            </Show>
          </div>
        </Show>
      </Show>
      </Show>
      </Show>

      <Show when={spot() && lib()}>
        <Spotlight libraryId={lib()!.id} kind={props.kind} filters={filters()} onClose={() => setSpot(false)} onClearFilters={clearFilters} />
      </Show>
      <Show when={panel()}>
        <FilterPanel kind={props.kind} filters={filters()} genres={genres() ?? []} onChange={setFilters} onClear={clearFilters} onClose={() => setPanel(false)} />
      </Show>
      <Show when={sortSheet()}>
        <Sheet
          title="Sort by"
          options={SORTS.map((s) => ({ value: s.value, title: s.label, detail: s.detail }))}
          selected={sort()}
          onSelect={(v) => setSort(v)}
          onClose={() => setSortSheet(false)}
        />
      </Show>
    </div>
  );
}

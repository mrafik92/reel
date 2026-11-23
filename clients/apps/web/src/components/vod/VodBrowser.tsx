import { createEffect, createResource, createSignal, For, on, onCleanup, Show, type JSX } from "solid-js";
import { useLocation, useSearchParams } from "@solidjs/router";
import { Icon, Row, Sheet, VirtualGrid, isTvMode } from "@reel/ui";
import { vod, type VodCard as Card, type VodKind, type VodLane, type VodSort } from "../../api/vod";
import { href } from "../../lib/mode";
import { EmptyState, ErrorState, SkeletonRow } from "../States";
import { VodCard } from "./VodCard";
import { CategoryBar, CategoryLabel, CategorySheet } from "./Categories";

const SORTS: { value: VodSort; label: string; detail?: string }[] = [
  { value: "added", label: "Recently added", detail: "Newest on your provider first" },
  { value: "rating", label: "Highest rated" },
  { value: "year", label: "Newest first", detail: "By release year" },
  { value: "title", label: "A–Z" },
];
const PAGE = 60;

interface VodParams {
  source?: string;
  cat?: string;
  q?: string;
  genre?: string;
  sort?: string;
  view?: string;
  [k: string]: string | undefined;
}

interface GridState {
  items: Card[];
  cursor: string | null | undefined;
  done: boolean;
}
const gridCache = new Map<string, GridState>();

/**
 * The IPTV side of Movies / Shows: provider categories, curated lanes, a full grid with
 * sort + genre, and instant search — the same picker pattern as the library.
 */
export function VodBrowser(props: { kind: VodKind; switcher: JSX.Element; onReady: () => void }): JSX.Element {
  const [params, setParams] = useSearchParams<VodParams>();
  const loc = useLocation();
  const tv = isTvMode();
  const movies = props.kind === "movie";
  const noun = movies ? "films" : "series";

  const [cats, { refetch: refetchCats }] = createResource(() => props.kind, (k) => vod.categories(k));
  const [lanes] = createResource(() => props.kind, (k) => vod.lanes(k).catch(() => null as VodLane[] | null));
  const total = () => (cats() ?? []).reduce((a, c) => a + c.count, 0);
  const catId = () => (params.cat ? params.cat : null);
  const cat = () => (catId() == null ? undefined : (cats() ?? []).find((c) => String(c.id) === catId()));
  const q = () => (params.q ?? "").trim();
  const sort = (): VodSort => (SORTS.some((s) => s.value === params.sort) ? (params.sort as VodSort) : q() ? "title" : "added");
  const genre = () => params.genre || undefined;
  const lanesOff = () => !lanes.loading && (!lanes() || lanes()!.every((l) => !l.items.length));
  const grid = () => params.view === "all" || !!catId() || !!q() || !!genre() || !!params.sort || lanesOff();
  const nActive = () => (catId() ? 1 : 0) + (q() ? 1 : 0) + (genre() ? 1 : 0);

  const [catalogGenres] = createResource(
    () => props.kind,
    (kind) => vod.genres(kind).catch(() => []),
  );
  const genres = () => {
    const choices = catalogGenres() ?? [];
    const selected = genre();
    return selected && !choices.includes(selected) ? [selected, ...choices] : choices;
  };

  // ---- Grid paging
  const [items, setItems] = createSignal<Card[]>([]);
  const [error, setError] = createSignal<unknown>();
  const [ready, setReady] = createSignal(false);
  const [done, setDone] = createSignal(false);
  let state: GridState = { items: [], cursor: undefined, done: false };
  let loading = false;
  let key = "";

  const loadMore = async () => {
    if (loading || state.done) return;
    loading = true;
    const myKey = key;
    try {
      const page = await vod.list(props.kind, { category: catId(), q: q() || undefined, genre: genre(), sort: sort(), cursor: state.cursor, limit: PAGE });
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
    on([catId, q, genre, sort, grid], ([c, qq, g, s, gr]) => {
      if (!gr) return;
      const k = `${props.kind}:${c ?? ""}:${qq}:${g ?? ""}:${s}`;
      if (k === key) return;
      key = k;
      setError(undefined);
      const hit = gridCache.get(key);
      state = hit ?? { items: [], cursor: undefined, done: false };
      gridCache.set(key, state);
      setItems(state.items);
      setDone(state.done);
      setReady(false);
      loading = false;
      if (state.items.length || state.done) setReady(true);
      else void loadMore();
    }),
  );
  const onRange = (_a: number, end: number) => {
    if (end + PAGE / 2 > state.items.length) void loadMore();
  };
  const gridTotal = () => {
    if (done()) return items().length;
    if (!q() && !genre()) return Math.max(cat() ? cat()!.count : total(), items().length);
    return items().length + 12;
  };

  createEffect(() => {
    if (grid() ? ready() || !!error() : !lanes.loading) props.onReady();
  });

  // ---- Search (instant, debounced into the URL)
  const [typed, setTyped] = createSignal(params.q ?? "");
  let tSearch: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => tSearch && clearTimeout(tSearch));
  const onSearch = (v: string) => {
    setTyped(v);
    if (tSearch) clearTimeout(tSearch);
    tSearch = setTimeout(() => setParams({ q: v.trim() || undefined }, { replace: true }), 260);
  };
  const clearSearch = () => {
    setTyped("");
    if (tSearch) clearTimeout(tSearch);
    setParams({ q: undefined }, { replace: true });
  };

  // ---- URL helpers
  const setCat = (id: string | number | null) => setParams({ cat: id == null ? undefined : String(id) }, { replace: grid() });
  const setView = (all: boolean) => setParams(all ? { view: "all" } : { view: undefined, cat: undefined, q: undefined, genre: undefined, sort: undefined });
  const setSort = (s: VodSort) => setParams({ sort: s === "added" && !q() ? undefined : s, view: "all" }, { replace: true });
  const clearAll = () => {
    setTyped("");
    setParams({ cat: undefined, q: undefined, genre: undefined }, { replace: true });
  };
  /** Lane "See all" → this page with the lane's query applied. */
  const laneHref = (l: VodLane): string | undefined => {
    if (!l.see_all) return undefined;
    const p = new URLSearchParams(l.see_all.query);
    const out = new URLSearchParams();
    out.set("source", "iptv");
    out.set("view", "all");
    const c = p.get("category");
    if (c) out.set("cat", c);
    const s = p.get("sort");
    if (s) out.set("sort", s);
    const g = p.get("genre");
    if (g) out.set("genre", g);
    return href(`${loc.pathname}?${out.toString()}`);
  };

  const [catSheet, setCatSheet] = createSignal(false);
  const [sortSheet, setSortSheet] = createSignal(false);
  const [genreSheet, setGenreSheet] = createSignal(false);
  const sortLabel = () => SORTS.find((s) => s.value === sort())!.label;
  const found = () => {
    const n = done() ? items().length : !q() && !genre() && cat() ? cat()!.count : -1;
    if (n < 0) return q() ? "Searching…" : "Loading…";
    return `${n.toLocaleString()} ${n === 1 ? (movies ? "film" : "series") : noun}`;
  };

  return (
    <>
      <header class="page-head picker vpicker">
        {props.switcher}
        <div class="page-head__row picker__title-row">
          <h1 class="page-title">
            {movies ? "Films" : "Series"}
            <Show when={cats()}>
              <span class="page-title__count">
                {total().toLocaleString()} on IPTV · {(cats() ?? []).length} categories
              </span>
            </Show>
          </h1>
        </div>

        <Show when={cats() && total() > 0}>
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
            <Show when={genres().length}>
              <button type="button" class="btn btn--select" onClick={() => setGenreSheet(true)}>
                <Icon name="layers" /> <span class="btn__label">Genre</span> <span class="btn__value">{genre() ?? "Any"}</span>
              </button>
            </Show>
            <span class="picker__spacer" />
            <Show when={grid()}>
              <button type="button" class="btn btn--ghost picker__sort" onClick={() => setSortSheet(true)}>
                <Icon name="sort" /> <span class="btn__label">Sort</span> <span class="btn__value">{sortLabel()}</span>
              </button>
            </Show>
            <label class="vsearch" classList={{ "has-value": !!typed() }}>
              <Icon name="search" class="vsearch__icon" />
              <input
                class="vsearch__input"
                type="search"
                placeholder={`Search IPTV ${noun}`}
                value={typed()}
                onInput={(e) => onSearch(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    if (tSearch) clearTimeout(tSearch);
                    setParams({ q: typed().trim() || undefined }, { replace: true });
                  }
                }}
                autocomplete="off"
                spellcheck={false}
                enterkeyhint="search"
                aria-label={`Search IPTV ${noun}`}
              />
              <Show when={typed()}>
                <button type="button" class="iconbtn vsearch__clear" aria-label="Clear search" onClick={clearSearch}>
                  <Icon name="x" />
                </button>
              </Show>
            </label>
          </div>

          <CategoryBar categories={cats()!} active={catId()} browsing={!grid()} onPick={setCat} onMore={() => setCatSheet(true)} />

          <Show when={grid() && nActive()}>
            <div class="picker__active" data-nav-group>
              <span class="picker__found" aria-live="polite">{found()}</span>
              <Show when={q()}>
                <button type="button" class="fchip" onClick={clearSearch} aria-label={`Clear search ${q()}`}>
                  “<span dir="auto">{q()}</span>” <Icon name="x" />
                </button>
              </Show>
              <Show when={cat()}>
                <button type="button" class="fchip fchip--cat" onClick={() => setCat(null)} aria-label={`Remove category ${cat()!.name}`}>
                  <CategoryLabel name={cat()!.name} /> <Icon name="x" />
                </button>
              </Show>
              <Show when={genre()}>
                <button type="button" class="fchip" onClick={() => setParams({ genre: undefined }, { replace: true })} aria-label={`Remove ${genre()}`}>
                  {genre()} <Icon name="x" />
                </button>
              </Show>
              <Show when={nActive() > 1}>
                <button type="button" class="fchip fchip--clear" onClick={clearAll}>Clear all</button>
              </Show>
            </div>
          </Show>
        </Show>
      </header>

      <Show when={cats.error}>
        <div class="library__body">
          <ErrorState error={cats.error} title="Couldn't load your IPTV catalogue" onRetry={refetchCats} />
        </div>
      </Show>
      <Show when={cats() && total() === 0}>
        <div class="library__body">
          <EmptyState title={`No ${noun} on your IPTV provider yet`} body="The catalogue fills in after the provider's next refresh. Live TV is unaffected." />
        </div>
      </Show>

      {/* ---- Browse: lanes ---- */}
      <Show when={cats() && total() > 0 && !grid()}>
        <Show when={!lanes.loading} fallback={<div class="library__lanes"><SkeletonRow /><SkeletonRow /></div>}>
          <div class="library__lanes">
            <For each={(lanes() ?? []).filter((l) => l.items.length)}>
              {(lane, i) => (
                <Row
                  title={lane.title}
                  subtitle={lane.subtitle}
                  class={`row--lane vlane vlane--${lane.kind}`}
                  action={laneHref(lane) ? { label: "Explore all", href: laneHref(lane)! } : undefined}
                >
                  <For each={lane.items}>
                    {(it, j) => <VodCard card={it} badge={false} autofocus={i() === 0 && j() === 0} />}
                  </For>
                </Row>
              )}
            </For>
            <div class="lane-cta">
              <p>
                <strong>Looking for something?</strong> Search all {total().toLocaleString()} {noun}, or dig through the provider's categories.
              </p>
              <button type="button" class="btn" onClick={() => setCatSheet(true)}>
                <Icon name="search" /> <span>Find a category</span>
              </button>
            </div>
          </div>
        </Show>
      </Show>

      {/* ---- Grid ---- */}
      <Show when={cats() && total() > 0 && grid()}>
        <div class="library__body">
          <Show when={error()}>
            <ErrorState error={error()} onRetry={() => { setError(undefined); void loadMore(); }} />
          </Show>
          <Show when={!error()}>
            <Show
              when={!(done() && items().length === 0)}
              fallback={
                <EmptyState
                  title={q() ? `Nothing on IPTV called “${q()}”` : "Nothing here"}
                  body={q() && cat() ? "That search only looked in one category." : q() ? "Try fewer words, or the original-language title — providers often list Arabic titles in Latin letters, or the other way round." : "This category is empty right now."}
                >
                  <Show when={cat() && q()} fallback={
                    <button type="button" class="btn btn--primary" onClick={clearAll} data-autofocus>
                      <Icon name="x" /> <span>Clear search</span>
                    </button>
                  }>
                    <button type="button" class="btn btn--primary" onClick={() => setCat(null)} data-autofocus>
                      <Icon name="search" /> <span>Search every category</span>
                    </button>
                  </Show>
                </EmptyState>
              }
            >
              <VirtualGrid
                class="library__grid"
                items={items()}
                total={gridTotal()}
                minColumnRem={tv ? 8.4 : 10.5}
                ratio={1.5}
                captionRem={tv ? 3 : 3.1}
                gapRem={tv ? 1.25 : 1.5}
                onRange={onRange}
                renderItem={(it, i) => <VodCard card={it} badge={false} autofocus={i() === 0} />}
                renderPlaceholder={() => (
                  <div class="card card--poster is-placeholder">
                    <div class="card__frame skel" />
                  </div>
                )}
              />
            </Show>
          </Show>
        </div>
      </Show>

      <Show when={catSheet() && cats()}>
        <CategorySheet categories={cats()!} active={catId()} noun={noun} onPick={(id) => setParams({ cat: id == null ? undefined : String(id), view: "all" })} onClose={() => setCatSheet(false)} />
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
      <Show when={genreSheet()}>
        <Sheet
          title="Genre"
          options={[{ value: "", title: "Any genre" }].concat(genres().map((g) => ({ value: g, title: g })))}
          selected={genre() ?? ""}
          onSelect={(v) => setParams({ genre: v || undefined, view: "all" }, { replace: true })}
          onClose={() => setGenreSheet(false)}
        />
      </Show>
    </>
  );
}


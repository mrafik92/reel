import { createEffect, createResource, createSignal, For, on, Show, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import {
  PosterCard, VirtualGrid, isTvMode,
  type ItemSummary, type Library as Lib, type LibraryKind, type LibrarySort,
} from "@reel/ui";
import { api } from "../api";
import { usePageFocus } from "../lib/focus";
import { itemHref, libraryHref } from "../lib/links";
import { EmptyState, ErrorState } from "../components/States";

const SORTS: { value: LibrarySort; label: string }[] = [
  { value: "title", label: "A–Z" },
  { value: "added", label: "Recently added" },
  { value: "year", label: "Year" },
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
  const [params, setParams] = useSearchParams<{ lib?: string; sort?: string }>();
  const [libs, { refetch }] = createResource(() => api.libraries());
  const ofKind = () => (libs() ?? []).filter((l) => l.kind === props.kind);
  const lib = (): Lib | undefined => ofKind().find((l) => String(l.id) === params.lib) ?? ofKind()[0];
  const sort = (): LibrarySort =>
    params.sort === "added" || params.sort === "year" ? params.sort : "title";

  const [items, setItems] = createSignal<ItemSummary[]>([]);
  const [error, setError] = createSignal<unknown>();
  const [ready, setReady] = createSignal(false);
  let state: GridState = { items: [], cursor: undefined, done: false };
  let loading = false;
  let key = "";

  const loadMore = async () => {
    const l = lib();
    if (!l || loading || state.done) return;
    loading = true;
    const myKey = key;
    try {
      const page = await api.libraryItems(l.id, { sort: sort(), cursor: state.cursor, limit: PAGE });
      if (myKey !== key) return;
      state.items = state.items.concat(page.items);
      state.cursor = page.next_cursor;
      state.done = !page.next_cursor;
      setItems(state.items);
      setReady(true);
    } catch (e) {
      if (myKey === key) setError(e);
    } finally {
      loading = false;
    }
  };

  createEffect(
    on([lib, sort], ([l, s]) => {
      if (!l) return;
      key = `${l.id}:${s}`;
      setError(undefined);
      const hit = gridCache.get(key);
      state = hit ?? { items: [], cursor: undefined, done: false };
      gridCache.set(key, state);
      setItems(state.items);
      loading = false;
      if (state.items.length) setReady(true);
      else void loadMore();
    }),
  );

  const onRange = (_a: number, end: number) => {
    if (end + PAGE / 2 > state.items.length) void loadMore();
  };

  let el!: HTMLDivElement;
  usePageFocus(() => ready() || (!!libs() && !lib()), () => el);

  const noun = () => (props.kind === "shows" ? "series" : "films");
  const tv = isTvMode();

  return (
    <div class="page library" ref={el}>
      <Show when={!libs.error} fallback={<ErrorState error={libs.error} onRetry={refetch} />}>
        <header class="page-head">
          <Show when={ofKind().length > 1}>
            <nav class="tabs" aria-label="Libraries" data-nav-group>
              <For each={ofKind()}>
                {(l) => (
                  <a class="tab" href={libraryHref(l)} aria-current={l.id === lib()?.id ? "page" : undefined}>
                    {l.name}
                  </a>
                )}
              </For>
            </nav>
          </Show>
          <div class="page-head__row">
            <h1 class="page-title">
              {lib()?.name ?? (props.kind === "shows" ? "Shows" : "Movies")}
              <Show when={lib()}>
                <span class="page-title__count">
                  {lib()!.item_count.toLocaleString()} {noun()}
                </span>
              </Show>
            </h1>
            <div class="segmented" role="group" aria-label="Sort by" data-nav-group>
              <For each={SORTS}>
                {(s) => (
                  <button
                    type="button"
                    class="segmented__btn"
                    aria-pressed={sort() === s.value}
                    data-nav-default={sort() === s.value ? "" : undefined}
                    onClick={() => setParams({ sort: s.value === "title" ? undefined : s.value }, { replace: true })}
                  >
                    {s.label}
                  </button>
                )}
              </For>
            </div>
          </div>
        </header>

        <Show when={libs() && !lib()}>
          <EmptyState
            title={`No ${props.kind === "shows" ? "TV" : "movie"} libraries yet`}
            body="Add a library on the server and run a scan — titles will appear here as they're matched."
          />
        </Show>

        <Show when={error()}>
          <ErrorState error={error()} onRetry={() => { setError(undefined); void loadMore(); }} />
        </Show>

        <Show when={lib() && !error()}>
          <Show
            when={lib()!.item_count > 0 || items().length > 0}
            fallback={<EmptyState title="This library is empty" body="Run a scan on the server to pick up new files." />}
          >
            <VirtualGrid
              class="library__grid"
              items={items()}
              total={Math.max(lib()!.item_count, items().length)}
              minColumnRem={tv ? 8.4 : 10.5}
              ratio={1.5}
              captionRem={tv ? 3 : 3.1}
              gapRem={tv ? 1.25 : 1.5}
              onRange={onRange}
              renderItem={(it, i) => <PosterCard item={it} href={itemHref(it)} autofocus={i() === 0} />}
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
  );
}

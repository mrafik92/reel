import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { Icon, PosterCard, focusElement } from "@reel/ui";
import { api, ApiError, type DiscoverCard } from "../api";
import { itemHref } from "../lib/links";
import { usePageFocus } from "../lib/focus";
import { primeFromCards } from "../lib/downloads";
import { EmptyState, ErrorState } from "../components/States";
import { DiscoverTile, useDiscoverSheet } from "../components/Discover";

const SUGGESTIONS = ["Lantern", "Harbour", "Night", "Comet", "Glass", "Kingfisher"];

export default function Search(): JSX.Element {
  const [params, setParams] = useSearchParams<{ q?: string }>();
  const [q, setQ] = createSignal(params.q ?? "");
  const [term, setTerm] = createSignal(q().trim());
  const [library] = createResource(term, (t) => (t ? api.search(t, 40) : Promise.resolve([])));
  // TMDB: settle the library first so the viewer sees what they already have.
  const [tmdb] = createResource(term, async (t): Promise<DiscoverCard[] | "off"> => {
    if (!t) return [];
    try {
      const r = await api.discoverSearch(t);
      primeFromCards(r);
      return r.filter((c) => c.status !== "available");
    } catch (e) {
      if (e instanceof ApiError && e.status === 424) return "off";
      throw e;
    }
  });
  const sheet = useDiscoverSheet();

  let input!: HTMLInputElement;
  let el!: HTMLDivElement;
  let t: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => t && clearTimeout(t));

  const onInput = (v: string) => {
    setQ(v);
    if (t) clearTimeout(t);
    t = setTimeout(() => {
      setTerm(v.trim());
      setParams({ q: v.trim() || undefined }, { replace: true });
    }, 300);
  };
  const choose = (s: string) => {
    if (input) input.value = s;
    setQ(s);
    setTerm(s);
    setParams({ q: s }, { replace: true });
  };

  usePageFocus(() => true, () => el);
  onMount(() => {
    if (!params.q) requestAnimationFrame(() => focusElement(input, { smooth: false }));
  });

  const libCount = () => library()?.length ?? 0;
  const tmdbOff = () => !tmdb.error && tmdb() === "off";
  const tmdbList = () => (tmdb.error || tmdb.loading ? [] : Array.isArray(tmdb()) ? (tmdb() as DiscoverCard[]) : []);

  return (
    <div class="page search" ref={el}>
      <header class="search__head">
        <label class="search__field">
          <Icon name="search" class="search__icon" />
          <input
            ref={input}
            class="search__input"
            type="search"
            placeholder="Search films and series"
            value={q()}
            onInput={(e) => onInput(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                if (t) clearTimeout(t);
                setTerm(q().trim());
              }
            }}
            autocomplete="off"
            autocapitalize="off"
            spellcheck={false}
            data-autofocus={params.q ? undefined : ""}
            aria-label="Search your library and TMDB"
          />
        </label>
        <p class="search__hint">
          <Show when={term()} fallback="Search your library, plus everything you could add to it.">
            <Show when={!library.loading} fallback="Searching…">
              {libCount()} in your library{tmdb.loading ? " · looking further…" : tmdbList().length ? ` · ${tmdbList().length} you can add` : ""}
            </Show>
          </Show>
        </p>
      </header>

      <Show when={!term()}>
        <div class="search__suggest" data-nav-group>
          <p class="eyebrow">Try</p>
          <div class="chips">
            <For each={SUGGESTIONS}>{(s) => <button type="button" class="chip" onClick={() => choose(s)}>{s}</button>}</For>
          </div>
        </div>
      </Show>

      <Show when={library.error}>
        <ErrorState error={library.error} />
      </Show>

      <Show when={term() && library() && !library.error}>
        <Show when={libCount()}>
          <section class="search__section">
            <h2 class="section-title">In your library</h2>
            <div class="results-grid" data-nav-group="free">
              <For each={library()}>{(it) => <PosterCard item={it} href={itemHref(it)} />}</For>
            </div>
          </section>
        </Show>
        <Show when={!tmdbOff() && (tmdb.loading || tmdbList().length)}>
          <section class="search__section">
            <h2 class="section-title">
              Add to your library <span class="section-title__aside">from TMDB</span>
            </h2>
            <Show when={!tmdb.loading} fallback={<div class="results-grid"><div class="card card--poster is-placeholder"><div class="card__frame skel" /></div></div>}>
              <div class="results-grid results-grid--discover" data-nav-group="free">
                <For each={tmdbList()}>{(c) => <DiscoverTile card={c} onOpen={sheet.open} />}</For>
              </div>
            </Show>
          </section>
        </Show>
        <Show when={!libCount() && !tmdb.loading && !tmdbList().length}>
          <EmptyState title="Nothing matches that" body="Check the spelling, or try a shorter part of the title." />
        </Show>
      </Show>
      {sheet.view()}
    </div>
  );
}

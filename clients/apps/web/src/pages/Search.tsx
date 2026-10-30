import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { Icon, Img, PosterCard, Row, episodeCode, focusElement, focusFirst, isTvMode, type ItemSummary } from "@reel/ui";
import { api, ApiError, type DiscoverCard, type PersonHit, type SearchAll } from "../api";
import { itemHref } from "../lib/links";
import { href, MOCK } from "../lib/mode";
import { usePageFocus } from "../lib/focus";
import { itemActivity, primeFromCards } from "../lib/downloads";
import { EmptyState, ErrorState } from "../components/States";
import { DiscoverTile, useDiscoverSheet } from "../components/Discover";
import { clearRecent, forgetRecent, recentSearches, rememberSearch } from "../lib/recent";
import { VodCard } from "../components/vod/VodCard";
import type { SearchAllVod, VodCard as VodHit } from "../api/vod";

const MOCK_SUGGESTIONS = ["Lantern", "Harbour", "Night", "Comet", "Glass", "Kingfisher"];
const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

interface Full {
  t: string;
  r: SearchAll;
  /** The server predates /search/all: library + TMDB only. */
  legacy?: boolean;
}

export default function Search(): JSX.Element {
  const [params, setParams] = useSearchParams<{ q?: string }>();
  const initial = (params.q ?? "").trim();
  const [q, setQ] = createSignal(params.q ?? "");
  const [term, setTerm] = createSignal(initial);
  const [quick, setQuick] = createSignal<{ t: string; items: ItemSummary[] }>();
  const [full, setFull] = createSignal<Full>();
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<unknown>();
  const [tmdbOff, setTmdbOff] = createSignal(false);
  let legacy = false;
  const sheet = useDiscoverSheet();
  const tv = isTvMode();

  // Empty state: the genres people actually have.
  const [genres] = createResource(() => api.genres().catch(() => []));

  let input!: HTMLInputElement;
  let el!: HTMLDivElement;
  let results: HTMLDivElement | undefined;
  let tQuick: ReturnType<typeof setTimeout> | undefined;
  let tFull: ReturnType<typeof setTimeout> | undefined;
  let focusOnLand = false;
  onCleanup(() => {
    if (tQuick) clearTimeout(tQuick);
    if (tFull) clearTimeout(tFull);
  });

  const runQuick = (t: string) => {
    if (!t) return setQuick(undefined);
    api.search(t, 30).then((items) => q().trim() === t && setQuick({ t, items }), () => undefined);
  };

  const runFull = async (t: string) => {
    setTerm(t);
    setParams({ q: t || undefined }, { replace: true });
    if (!t) {
      setFull(undefined);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(undefined);
    try {
      let r: Full | undefined;
      if (!legacy) {
        try {
          r = { t, r: await api.searchAll(t, 30) };
        } catch (e) {
          if (!(e instanceof ApiError && (e.status === 404 || e.status === 405))) throw e;
          legacy = true;
        }
      }
      if (legacy) r = await legacySearch(t);
      if (term() !== t) return;
      primeFromCards(r!.r.discover);
      setFull(r!);
      if (focusOnLand) {
        focusOnLand = false;
        requestAnimationFrame(() => results && focusFirst(results));
      }
    } catch (e) {
      if (term() === t) setError(e);
    } finally {
      if (term() === t) setLoading(false);
    }
  };

  const legacySearch = async (t: string): Promise<Full> => {
    const [library, tm] = await Promise.all([
      api.search(t, 40),
      api.discoverSearch(t).then(
        (r) => r.filter((c) => c.status !== "available"),
        (e) => {
          if (e instanceof ApiError && e.status === 424) setTmdbOff(true);
          return [] as DiscoverCard[];
        },
      ),
    ]);
    return { t, legacy: true, r: { library, episodes: [], people: [], discover: tm } };
  };

  const onInput = (v: string) => {
    setQ(v);
    const t = v.trim();
    if (tQuick) clearTimeout(tQuick);
    if (tFull) clearTimeout(tFull);
    // Library titles come back almost instantly; the wider search waits for a pause.
    tQuick = setTimeout(() => runQuick(t), 90);
    tFull = setTimeout(() => void runFull(t), 250);
  };
  const commit = (s: string) => {
    if (input) input.value = s;
    setQ(s);
    if (tQuick) clearTimeout(tQuick);
    if (tFull) clearTimeout(tFull);
    runQuick(s.trim());
    void runFull(s.trim());
    if (s.trim()) rememberSearch(s.trim());
  };

  usePageFocus(() => true, () => el);
  onMount(() => {
    if (initial) {
      runQuick(initial);
      void runFull(initial);
    } else requestAnimationFrame(() => focusElement(input, { smooth: false }));
  });

  // What to show for the current text.
  const typed = () => q().trim();
  const lib = (): ItemSummary[] => {
    const f = full();
    if (f && f.t === typed()) return f.r.library;
    const qk = quick();
    if (qk && qk.t === typed()) return qk.items;
    return f?.r.library ?? qk?.items ?? [];
  };
  const stale = () => !!full() && full()!.t !== typed();
  const eps = () => full()?.r.episodes ?? [];
  const people = () => full()?.r.people ?? [];
  const disc = () => full()?.r.discover ?? [];
  /** IPTV on-demand hits; titles already shown as library results are left out. */
  const vodHits = (): VodHit[] => {
    const all = (full()?.r as SearchAllVod | undefined)?.vod ?? [];
    const shown = new Set(lib().map((x) => x.id));
    return all.filter((v) => !v.library_item_id || !shown.has(v.library_item_id));
  };
  const peopleFirst = () => people().some((p) => p.in_library_count > 0);
  const peopleRow = () => (
    <Show when={people().length}>
      <Row title="People" subtitle={String(people().length)} variant="cast" class="row--results">
        <For each={people()}>{(p) => <PersonHitTile p={p} />}</For>
      </Row>
    </Show>
  );
  const anything = () => lib().length + eps().length + people().length + disc().length + vodHits().length > 0;
  const settled = () => !loading() && !!full() && full()!.t === typed();

  const hint = () => {
    if (!typed()) return "";
    if (!settled()) return "Searching…";
    const bits: string[] = [];
    if (lib().length) bits.push(`${lib().length} in your library`);
    if (eps().length) bits.push(`${eps().length} episode${eps().length === 1 ? "" : "s"}`);
    if (people().length) bits.push(`${people().length} ${people().length === 1 ? "person" : "people"}`);
    if (vodHits().length) bits.push(`${vodHits().length} on IPTV`);
    if (disc().length) bits.push(`${disc().length} you can add`);
    return bits.join(" · ");
  };

  // Clicking any result counts as a search worth remembering.
  const onResultsClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).closest("a,button") && typed()) rememberSearch(typed());
  };

  return (
    <div class="page search" ref={el}>
      <header class="search__head">
        <label class="search__field">
          <Icon name="search" class="search__icon" />
          <input
            ref={input}
            class="search__input"
            type="search"
            placeholder="Films, series, episodes, people"
            value={q()}
            onInput={(e) => onInput(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.keyCode === 13) {
                e.preventDefault();
                focusOnLand = tv;
                commit(q());
              }
            }}
            autocomplete="off"
            autocapitalize="off"
            spellcheck={false}
            enterkeyhint="search"
            inputmode="search"
            data-autofocus={params.q ? undefined : ""}
            aria-label="Search films, series, episodes and people"
          />
          <Show when={q()}>
            <button type="button" class="iconbtn search__clear" aria-label="Clear search" onClick={() => { commit(""); focusElement(input, { smooth: false }); }}>
              <Icon name="x" />
            </button>
          </Show>
        </label>
        <p class="search__hint">
          <Show
            when={typed()}
            fallback={
              <Show when={!tv} fallback="Type a title, an actor or an episode — then press Down to browse the results.">
                Search your library, plus everything you could add to it.{" "}
                <span class="search__keys">
                  Press <span class="keycap">/</span> or <span class="keycap">{isMac ? "⌘" : "Ctrl"} K</span> anywhere.
                </span>
              </Show>
            }
          >
            {hint()}
          </Show>
        </p>
      </header>

      <Show when={!typed()}>
        <div class="search__start">
          <Show when={recentSearches().length}>
            <section class="search__group" data-nav-group>
              <div class="search__group-head">
                <p class="eyebrow">Recent searches</p>
                <button type="button" class="linkbtn" onClick={clearRecent}>Clear</button>
              </div>
              <div class="chips">
                <For each={recentSearches()}>
                  {(s) => (
                    <span class="chip chip--recent">
                      <button type="button" class="chip__main" onClick={() => commit(s)}>
                        <Icon name="clock" /> {s}
                      </button>
                      <button type="button" class="chip__x" aria-label={`Forget ${s}`} onClick={() => forgetRecent(s)}>
                        <Icon name="x" />
                      </button>
                    </span>
                  )}
                </For>
              </div>
            </section>
          </Show>
          <Show when={(genres() ?? []).length}>
            <section class="search__group" data-nav-group>
              <p class="eyebrow">Browse by genre</p>
              <div class="chips">
                <For each={(genres() ?? []).slice(0, 14)}>
                  {(g) => (
                    <a class="chip chip--genre" href={href(`/movies?genre=${encodeURIComponent(g.name)}`)}>
                      {g.name} <span class="gchip__count">{g.count}</span>
                    </a>
                  )}
                </For>
              </div>
            </section>
          </Show>
          <Show when={MOCK}>
            <section class="search__group" data-nav-group>
              <p class="eyebrow">Try</p>
              <div class="chips">
                <For each={MOCK_SUGGESTIONS}>{(s) => <button type="button" class="chip" onClick={() => commit(s)}>{s}</button>}</For>
              </div>
            </section>
          </Show>
        </div>
      </Show>

      <Show when={error() && typed()}>
        <div class="search__pad">
          <ErrorState error={error()} onRetry={() => void runFull(typed())} />
        </div>
      </Show>

      <Show when={typed() && !error()}>
        <div class="search__results" classList={{ "is-stale": stale() }} ref={results} onClick={onResultsClick}>
          <Show when={lib().length}>
            <Row title="In your library" subtitle={String(lib().length)} class="row--results">
              <For each={lib()}>{(it) => <PosterCard item={it} href={itemHref(it)} activity={itemActivity(it.id)} />}</For>
            </Row>
          </Show>
          <Show when={eps().length}>
            <Row title="Episodes" subtitle={String(eps().length)} variant="landscape" class="row--results">
              <For each={eps()}>
                {(it) => <PosterCard item={it} variant="landscape" href={itemHref(it)} subtitle={episodeCode(it)} />}
              </For>
            </Row>
          </Show>
          {/* People you actually have films with lead; otherwise titles you could add come first. */}
          <Show when={peopleFirst()}>{peopleRow()}</Show>
          <Show when={vodHits().length}>
            <Row title="On IPTV" subtitle="Streams from your provider" class="row--results row--vod">
              <For each={vodHits()}>{(v) => <VodCard card={v} />}</For>
            </Row>
          </Show>
          <Show when={disc().length}>
            <Row title="Not in your library" subtitle="Add it and watch the ring fill" class="row--results row--discover">
              <For each={disc()}>{(c) => <DiscoverTile card={c} onOpen={sheet.open} />}</For>
            </Row>
          </Show>
          <Show when={!peopleFirst()}>{peopleRow()}</Show>
          <Show when={settled() && !anything()}>
            <div class="search__pad">
              <EmptyState title="Nothing matches that" body="Check the spelling, or try a shorter part of the title or name." />
            </div>
          </Show>
          <Show when={settled() && tmdbOff() && !disc().length && anything()}>
            <p class="search__fine">Titles you could add aren't shown — the server isn't connected to TMDB.</p>
          </Show>
        </div>
      </Show>
      {sheet.view()}
    </div>
  );
}

function PersonHitTile(props: { p: PersonHit }): JSX.Element {
  const initials = () => props.p.name.split(" ").map((w) => w.charAt(0)).join("").slice(0, 2);
  const line = () =>
    props.p.in_library_count
      ? `${props.p.in_library_count} in your library`
      : props.p.known_for.slice(0, 2).join(", ") || props.p.known_for_department || "";
  return (
    <a
      class="person person--link person--hit"
      href={href(`/person/${props.p.tmdb_id}`)}
      data-fid={"ph:" + props.p.tmdb_id}
      aria-label={`${props.p.name}${line() ? ", " + line() : ""}`}
    >
      <Img class="person__photo" src={props.p.photo_url} width={185} fallback={<span class="person__initials">{initials()}</span>} />
      <span class="person__name">{props.p.name}</span>
      <Show when={line()}>
        <span class="person__role" classList={{ "person__role--lib": props.p.in_library_count > 0 }}>{line()}</span>
      </Show>
    </a>
  );
}

import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { Icon, PosterCard, focusElement } from "@reel/ui";
import { api } from "../api";
import { itemHref } from "../lib/links";
import { usePageFocus } from "../lib/focus";
import { EmptyState, ErrorState } from "../components/States";

const SUGGESTIONS = ["Lantern", "Harbour", "Night", "Summer", "Kingdom", "Machine"];

export default function Search(): JSX.Element {
  const [params, setParams] = useSearchParams<{ q?: string }>();
  const [q, setQ] = createSignal(params.q ?? "");
  const [term, setTerm] = createSignal(q().trim());
  const [results] = createResource(term, (t) => (t ? api.search(t) : Promise.resolve([])));

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
    }, 250);
  };
  const choose = (s: string) => {
    if (input) input.value = s;
    setQ(s);
    setTerm(s);
    setParams({ q: s }, { replace: true });
  };

  // Land on the input first (TV keyboards open on Enter).
  usePageFocus(() => true, () => el);
  onMount(() => {
    if (!params.q) requestAnimationFrame(() => focusElement(input, { smooth: false }));
  });

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
            aria-label="Search your library"
          />
        </label>
        <p class="search__hint">
          <Show when={term()} fallback="Search across every library on this server.">
            <Show when={!results.loading} fallback="Searching…">
              {results()?.length ?? 0} {results()?.length === 1 ? "result" : "results"} for “{term()}”
            </Show>
          </Show>
        </p>
      </header>

      <Show when={!term()}>
        <div class="search__suggest" data-nav-group>
          <p class="eyebrow">Try</p>
          <div class="chips">
            <For each={SUGGESTIONS}>
              {(s) => (
                <button type="button" class="chip" onClick={() => choose(s)}>
                  {s}
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <Show when={results.error}>
        <ErrorState error={results.error} />
      </Show>

      <Show when={term() && results() && !results.error}>
        <Show
          when={results()!.length}
          fallback={<EmptyState title="Nothing matches that" body="Check the spelling, or try a shorter part of the title." />}
        >
          <div class="results-grid" data-nav-group="free">
            <For each={results()}>{(it) => <PosterCard item={it} href={itemHref(it)} />}</For>
          </div>
        </Show>
      </Show>
    </div>
  );
}

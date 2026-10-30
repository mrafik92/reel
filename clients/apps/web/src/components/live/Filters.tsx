import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Icon, isTvMode, onBack } from "@reel/ui";
import { favouriteCount, filter, filterKey, groups, query, setFilter, setQuery, totalChannels, type Filter } from "./store";

const fold = (s: string) => s.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");

/**
 * The groups row: search, Favourites, All channels, then every group — one horizontal row that
 * scrolls on the remote. Big providers have 150+ groups; the search narrows them as you type.
 */
export function Filters(props: { class?: string; label?: boolean }): JSX.Element {
  let row!: HTMLDivElement;
  const [q, setQ] = createSignal("");
  const active = (f: Filter) => filterKey(filter()) === filterKey(f);
  const pick = (f: Filter) => {
    setQuery("");
    setFilter(f);
  };
  const list = createMemo(() => {
    const t = fold(q().trim());
    return t ? groups().filter((g) => fold(g.name).indexOf(t) >= 0) : groups();
  });
  // Bring the selected group into view when the row first renders.
  onMount(() =>
    requestAnimationFrame(() => {
      const el = row?.querySelector<HTMLElement>('.livechip[aria-pressed="true"]');
      if (el && row && el.offsetLeft + el.offsetWidth > row.clientWidth) row.scrollLeft = Math.max(0, el.offsetLeft - row.clientWidth / 3);
    }),
  );
  const chip = (f: Filter, label: JSX.Element, count?: number) => (
    <button
      type="button"
      class="chip chip--toggle livechip"
      aria-pressed={active(f)}
      data-nav-default={active(f) ? "" : undefined}
      onClick={() => pick(f)}
    >
      {label}
      <Show when={count != null}><small class="livechip__n">{count!.toLocaleString()}</small></Show>
    </button>
  );
  return (
    <div class={"grouprow" + (props.class ? " " + props.class : "")}>
      <Show when={props.label !== false}>
        <p class="rowlabel">
          <span>Groups</span> <small>{groups().length}</small>
        </p>
      </Show>
      <div ref={row} class="livefilters" data-nav-scroll="x" data-nav-group role="toolbar" aria-label="Channel groups">
        <div class="livefilters__track">
          <Show when={groups().length > 6}>
            <label class="livesearch livesearch--chip" classList={{ "has-text": !!q() }}>
              <Icon name="search" />
              <input
                type="search"
                placeholder="Find a group"
                value={q()}
                onInput={(e) => setQ(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && list().length) pick({ kind: "group", name: list()[0]!.name });
                }}
                aria-label="Find a group"
                autocomplete="off"
                spellcheck={false}
              />
              <Show when={q()}>
                <button type="button" class="livesearch__clear" aria-label="Clear" onClick={() => setQ("")}><Icon name="x" /></button>
              </Show>
            </label>
          </Show>
          <Show when={!q()}>
            {chip({ kind: "favorites" }, <><Icon name="star" /> <span>Favourites</span></>, favouriteCount())}
            {chip({ kind: "all" }, <span>All channels</span>, totalChannels())}
          </Show>
          <For each={list()} fallback={<span class="livefilters__none">No group matches “{q()}”</span>}>
            {(g) => chip({ kind: "group", name: g.name }, <span class="livechip__name" dir="auto">{g.name}</span>, g.count)}
          </For>
        </div>
      </div>
    </div>
  );
}

/** Searchable list of every group. */
export function GroupPicker(props: { onPick: (f: Filter) => void; onClose: () => void }): JSX.Element {
  let box!: HTMLDivElement;
  let input!: HTMLInputElement;
  const [q, setQ] = createSignal("");
  const list = createMemo(() => {
    const t = fold(q().trim());
    return t ? groups().filter((g) => fold(g.name).indexOf(t) >= 0) : groups();
  });
  const cur = () => filterKey(filter());
  onMount(() => {
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(off);
    requestAnimationFrame(() => {
      // TVs: land on the current group (focusing the field would pop the on-screen keyboard).
      if (!isTvMode()) return input.focus();
      const el = box.querySelector<HTMLElement>('[aria-current="true"]') ?? box.querySelector<HTMLElement>(".gpick__item");
      el?.focus({ preventScroll: true });
      el?.scrollIntoView({ block: "center" });
    });
  });
  return (
    <>
      <div class="gpick__scrim" onClick={props.onClose} aria-hidden="true" />
      <div ref={box} class="gpick" role="dialog" aria-modal="true" aria-label="Channel groups" data-nav-trap>
        <header class="gpick__head">
          <div>
            <h2 class="gpick__title">Channel groups</h2>
            <p class="gpick__sub">{groups().length} groups · {totalChannels().toLocaleString()} channels</p>
          </div>
          <button type="button" class="iconbtn" aria-label="Close" onClick={props.onClose}><Icon name="x" /></button>
        </header>
        <label class="livesearch gpick__search">
          <Icon name="search" />
          <input
            ref={input}
            type="search"
            placeholder="Find a group"
            value={q()}
            onInput={(e) => setQ(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const first = list()[0];
                if (first) props.onPick({ kind: "group", name: first.name });
              }
            }}
            autocomplete="off"
            spellcheck={false}
          />
        </label>
        <div class="gpick__list" data-nav-scroll="y" data-nav-group>
          <Show when={!q().trim()}>
            <button type="button" class="gpick__item gpick__item--special" aria-current={cur() === "favorites" ? "true" : undefined} onClick={() => props.onPick({ kind: "favorites" })}>
              <Icon name="star" /> <span class="gpick__name">Favourites</span> <small>{favouriteCount()}</small>
            </button>
            <button type="button" class="gpick__item gpick__item--special" aria-current={cur() === "all" ? "true" : undefined} onClick={() => props.onPick({ kind: "all" })}>
              <Icon name="layers" /> <span class="gpick__name">All channels</span> <small>{totalChannels().toLocaleString()}</small>
            </button>
          </Show>
          <For each={list()} fallback={<p class="gpick__empty">No group matches “{q()}”.</p>}>
            {(g) => (
              <button
                type="button"
                class="gpick__item"
                aria-current={cur() === `g:${g.name}` ? "true" : undefined}
                onClick={() => props.onPick({ kind: "group", name: g.name })}
              >
                <span class="gpick__name" dir="auto">{g.name}</span> <small>{g.count.toLocaleString()}</small>
              </button>
            )}
          </For>
        </div>
      </div>
    </>
  );
}

/** Instant channel search over the whole provider. */
export function ChannelSearch(props: { class?: string }): JSX.Element {
  return (
    <label class={"livesearch" + (props.class ? " " + props.class : "")}>
      <Icon name="search" />
      <input
        type="search"
        placeholder={`Search ${totalChannels() ? totalChannels().toLocaleString() + " " : ""}channels`}
        value={query()}
        onInput={(e) => setQuery(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && query()) {
            e.preventDefault();
            e.stopPropagation();
            setQuery("");
          }
        }}
        aria-label="Search channels"
        autocomplete="off"
        spellcheck={false}
      />
      <Show when={query()}>
        <button type="button" class="livesearch__clear" aria-label="Clear search" onClick={() => setQuery("")}>
          <Icon name="x" />
        </button>
      </Show>
    </label>
  );
}

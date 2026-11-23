import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Icon, onRemoteKey } from "@reel/ui";
import { favouriteCount, filter, filterKey, groupPrefs, groups, query, setFilter, setQuery, totalChannels, type Filter } from "./store";
import { classify, prefixOf, shortName, type GroupInfo } from "./groups";
import { GroupBrowser, GroupMenu } from "./GroupBrowser";
import { pressHandlers } from "./press";


/**
 * The groups row on the Live page — short and personal: "All groups" (opens the full browser),
 * Favourites, the group you're on, pinned groups, recently watched ones, All channels.
 * OK selects; long-press OK / Info / right-click opens pin & hide. Green (or B) opens the browser.
 */
export function Filters(props: { class?: string; label?: boolean }): JSX.Element {
  let row!: HTMLDivElement;
  let browseBtn: HTMLButtonElement | undefined;
  const [browsing, setBrowsing] = createSignal(false);
  const [menu, setMenu] = createSignal<GroupInfo>();
  const active = (f: Filter) => filterKey(filter()) === filterKey(f);
  const pick = (f: Filter) => {
    setQuery("");
    setFilter(f);
  };
  const personal = createMemo(() => {
    const visible = new Map(groups().map((g) => [g.name, g.count]));
    const f = filter();
    const names: string[] = [];
    const add = (n: string) => visible.has(n) && names.indexOf(n) < 0 && names.push(n);
    if (f.kind === "group") add(f.name);
    groupPrefs().pinned.forEach(add);
    groupPrefs().watched.forEach((n) => names.length < 10 && add(n));
    return names.map((n) => classify(n, visible.get(n)!));
  });
  const pinned = (n: string) => groupPrefs().pinned.indexOf(n) >= 0;

  onMount(() => {
    requestAnimationFrame(() => {
      const el = row?.querySelector<HTMLElement>('.livechip[aria-pressed="true"]');
      if (el && row && el.offsetLeft + el.offsetWidth > row.clientWidth) row.scrollLeft = Math.max(0, el.offsetLeft - row.clientWidth / 3);
    });
    const offKeys = onRemoteKey((e) => {
      if (browsing() || menu() || document.querySelector("[data-nav-trap]")) return false;
      if (e.key === "green" || (e.original.key === "b" && !/^(INPUT|TEXTAREA)$/.test((e.original.target as HTMLElement)?.tagName ?? ""))) {
        setBrowsing(true);
        return true;
      }
      if (e.key === "info" || e.key === "red") {
        const name = (document.activeElement as HTMLElement | null)?.getAttribute("data-group");
        if (!name) return false;
        setMenu(classify(name, groups().find((g) => g.name === name)?.count ?? 0));
        return true;
      }
      return false;
    });
    onCleanup(offKeys);
  });

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
          <span>Your groups</span>
          <span class="rowlabel__hint">Hold OK on a group to pin or hide it</span>
        </p>
      </Show>
      <div ref={row} class="livefilters" data-nav-scroll="x" data-nav-group role="toolbar" aria-label="Channel groups">
        <div class="livefilters__track">
          <button ref={browseBtn} type="button" class="chip livechip livechip--browse" aria-haspopup="dialog" onClick={() => setBrowsing(true)}>
            <Icon name="grid" /> <span>All groups</span> <small class="livechip__n">{groups().length}</small>
          </button>
          {chip({ kind: "favorites" }, <><Icon name="star" /> <span>Favourites</span></>, favouriteCount())}
          <For each={personal()}>
            {(g) => {
              const f: Filter = { kind: "group", name: g.name };
              const h = pressHandlers(() => pick(f), () => setMenu(g));
              return (
                <button
                  type="button"
                  class="chip chip--toggle livechip"
                  aria-pressed={active(f)}
                  data-nav-default={active(f) ? "" : undefined}
                  data-group={g.name}
                  title={g.name}
                  {...h}
                >
                  <Show when={pinned(g.name)}><Icon name="star" class="livechip__pin" /></Show>
                  <Show when={prefixOf(g.name)}>{(p) => <span class="livechip__code">{p()}</span>}</Show>
                  <span class="livechip__name" dir="auto">{shortName(g.name)}</span>
                  <small class="livechip__n">{g.count.toLocaleString()}</small>
                </button>
              );
            }}
          </For>
          {chip({ kind: "all" }, <span>All channels</span>, totalChannels())}
        </div>
      </div>
      <Show when={browsing()}>
        <GroupBrowser
          onPick={(f) => {
            pick(f);
            setBrowsing(false);
            // Land on the chosen group's chip in the row.
            requestAnimationFrame(() => row?.querySelector<HTMLElement>('.livechip[aria-pressed="true"]')?.focus({ preventScroll: false }));
          }}
          onClose={() => {
            setBrowsing(false);
            requestAnimationFrame(() => browseBtn?.focus({ preventScroll: true }));
          }}
        />
      </Show>
      <Show when={menu()}>
        {(g) => (
          <GroupMenu
            g={g()}
            onOpen={() => {
              setMenu(undefined);
              pick({ kind: "group", name: g().name });
            }}
            onClose={() => setMenu(undefined)}
          />
        )}
      </Show>
    </div>
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

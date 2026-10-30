import { For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { channels, filter, filterKey, groups, setFilter, type Filter } from "./store";

/** All · Favourites · groups — a horizontal chip row (scrolls on the remote). */
export function Filters(props: { class?: string }): JSX.Element {
  const favCount = () => (channels() ?? []).filter((c) => c.favorite).length;
  const active = (f: Filter) => filterKey(filter()) === filterKey(f);
  const chip = (f: Filter, label: JSX.Element, count?: number) => (
    <button
      type="button"
      class="chip chip--toggle livechip"
      aria-pressed={active(f)}
      data-nav-default={active(f) ? "" : undefined}
      onClick={() => setFilter(f)}
    >
      {label}
      <Show when={count != null}><small class="livechip__n">{count}</small></Show>
    </button>
  );
  return (
    <div class={"livefilters" + (props.class ? " " + props.class : "")} data-nav-scroll="x" data-nav-group role="toolbar" aria-label="Filter channels">
      <div class="livefilters__track">
        {chip({ kind: "all" }, <span>All</span>, (channels() ?? []).length)}
        {chip({ kind: "favorites" }, <><Icon name="star" /> <span>Favourites</span></>, favCount())}
        <For each={groups()}>{(g) => chip({ kind: "group", name: g.name }, <span>{g.name}</span>, g.count)}</For>
      </div>
    </div>
  );
}

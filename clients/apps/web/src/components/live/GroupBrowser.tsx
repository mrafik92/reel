import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { focusElement, Icon, Img, isTvMode, mediaUrl, onBack, onRemoteKey } from "@reel/ui";
import {
  allGroups, favouriteCount, filter, filterKey, groupLogos, groupPrefs, hideGroup, isHidden, loadAll, pinGroup, totalChannels, type Filter,
} from "./store";
import { classify, facetMatches, foldText, REGIONS, shortName, TOPICS, type GroupInfo } from "./groups";
import { pressHandlers } from "./press";

type Sort = "type" | "az";
const SORT_KEY = "reel.live.groupSort";

interface Section {
  id: string;
  title: string;
  items: GroupInfo[];
  /** Hidden groups: shown dimmed, only to unhide them. */
  hidden?: boolean;
}

const HUE: Record<string, string> = {
  sports: "#c6f432", movies: "#ff8a3d", series: "#ff5a7a", news: "#5ad1ff", kids: "#ffd23f", religious: "#3dffb4", music: "#b48cff",
  documentary: "#7fd18b", entertainment: "#f3eee4", events: "#ff5a4e",
};
const tint = (g: GroupInfo) => HUE[g.topic?.id ?? ""] ?? "#8e94a8";

/**
 * Full-screen group browser for providers with 100+ groups: search, Pinned, Recently watched, then
 * every group in smart sections (topics, then regions) or A–Z. OK opens a group; long-press OK,
 * Info/Red, right-click or ⋯ opens pin / hide.
 */
export function GroupBrowser(props: { onPick: (f: Filter) => void; onClose: () => void }): JSX.Element {
  let box!: HTMLDivElement;
  let body!: HTMLDivElement;
  let search!: HTMLInputElement;
  const [q, setQ] = createSignal("");
  const [sort, setSortSig] = createSignal<Sort>(((): Sort => {
    try {
      return localStorage.getItem(SORT_KEY) === "az" ? "az" : "type";
    } catch {
      return "type";
    }
  })());
  const [showHidden, setShowHidden] = createSignal(false);
  const [menu, setMenu] = createSignal<GroupInfo>();
  const setSort = (s: Sort) => {
    setSortSig(s);
    try {
      localStorage.setItem(SORT_KEY, s);
    } catch {
      /* ignore */
    }
    body.scrollTop = 0;
  };

  // Logos for the mosaics come from the full channel list.
  void loadAll().catch(() => undefined);

  const infos = createMemo(() => allGroups().map((g) => classify(g.name, g.count)));
  const byName = createMemo(() => new Map(infos().map((g) => [g.name, g])));
  const cur = () => filterKey(filter());

  const sections = createMemo((): Section[] => {
    const all = infos();
    const hid = new Set(groupPrefs().hidden);
    const shown = all.filter((g) => !hid.has(g.name));
    const t = foldText(q().trim());
    if (t) {
      const words = t.split(/\s+/).filter(Boolean);
      // Name match first, then groups whose topic/region the words name ("sports", "رياضة", "arabic").
      const named = all.filter((g) => words.every((w) => g.folded.indexOf(w) >= 0));
      const seen = new Set(named.map((g) => g.name));
      const faceted = all.filter((g) => !seen.has(g.name) && words.every((w) => g.folded.indexOf(w) >= 0 || facetMatches(g, w)));
      const hits = named.concat(faceted);
      return [{ id: "results", title: hits.length ? `${hits.length} group${hits.length === 1 ? "" : "s"}` : "", items: hits }];
    }
    const out: Section[] = [];
    const m = byName();
    const pinned = groupPrefs().pinned.map((n) => m.get(n)).filter((g): g is GroupInfo => !!g && !hid.has(g.name));
    if (pinned.length) out.push({ id: "pinned", title: "Pinned", items: pinned });
    const pinnedSet = new Set(pinned.map((g) => g.name));
    const recent = groupPrefs()
      .watched.map((n) => m.get(n))
      .filter((g): g is GroupInfo => !!g && !hid.has(g.name) && !pinnedSet.has(g.name))
      .slice(0, 12);
    if (recent.length) out.push({ id: "recent", title: "Recently watched", items: recent });
    const az = (a: GroupInfo, b: GroupInfo) => a.name.localeCompare(b.name);
    if (sort() === "type") {
      TOPICS.forEach((f) => {
        const items = shown.filter((g) => g.topic === f).sort(az);
        if (items.length) out.push({ id: `t-${f.id}`, title: f.label, items });
      });
      REGIONS.forEach((f) => {
        const items = shown.filter((g) => !g.topic && g.region === f).sort(az);
        if (items.length) out.push({ id: `r-${f.id}`, title: f.label, items });
      });
      const rest = shown.filter((g) => !g.topic && !g.region).sort(az);
      if (rest.length) out.push({ id: "more", title: "More", items: rest });
    } else {
      const letters = new Map<string, GroupInfo[]>();
      shown.slice().sort(az).forEach((g) => letters.set(g.letter, (letters.get(g.letter) ?? []).concat(g)));
      Array.from(letters.keys())
        .sort((a, b) => (a === "#" ? 1 : b === "#" ? -1 : a === "ع" ? 1 : b === "ع" ? -1 : a.localeCompare(b)))
        .forEach((l) => out.push({ id: `l-${l}`, title: l === "ع" ? "Arabic script" : l, items: letters.get(l)! }));
    }
    const hidden = all.filter((g) => hid.has(g.name));
    if (hidden.length) out.push({ id: "hidden", title: `Hidden (${hidden.length})`, items: showHidden() ? hidden : [], hidden: true });
    return out;
  });

  // Sections keyed by id (strings) so a pin / hide doesn't remount every tile (focus would be lost).
  const sectionIds = createMemo(() => sections().map((x) => x.id), undefined, {
    equals: (a, b) => a.length === b.length && a.every((v, i) => v === b[i]),
  });
  const sectionById = createMemo(() => new Map(sections().map((x) => [x.id, x])));

  const jumpTo = (id: string) => {
    const sec = body.querySelector<HTMLElement>(`[data-section="${id}"]`);
    if (!sec) return;
    body.scrollTop = Math.max(0, sec.offsetTop - 8);
    const first = sec.querySelector<HTMLElement>(".gtile");
    if (first) first.focus({ preventScroll: true });
  };

  const open = (g: GroupInfo) => props.onPick({ kind: "group", name: g.name });

  onMount(() => {
    const offBack = onBack(() => {
      if (menu()) {
        closeMenu();
        return true;
      }
      if (q() && document.activeElement === search) {
        setQ("");
        return true;
      }
      props.onClose();
      return true;
    });
    // Info / Red on a tile opens its menu (no long-press needed).
    const offKeys = onRemoteKey((e) => {
      if (menu() || (e.key !== "info" && e.key !== "red")) return false;
      const name = (document.activeElement as HTMLElement | null)?.getAttribute("data-group");
      const g = name ? byName().get(name) : undefined;
      if (!g) return false;
      setMenu(g);
      return true;
    });
    onCleanup(() => {
      offBack();
      offKeys();
    });
    requestAnimationFrame(() => {
      if (!isTvMode()) return search.focus();
      const curEl = box.querySelector<HTMLElement>('.gtile[aria-current="true"]') ?? box.querySelector<HTMLElement>(".gtile");
      if (curEl) focusElement(curEl, { smooth: false });
    });
  });

  let menuFrom: HTMLElement | null = null;
  let menuFor = "";
  const openMenu = (g: GroupInfo) => {
    menuFrom = document.activeElement as HTMLElement | null;
    menuFor = g.name;
    setMenu(g);
  };
  const closeMenu = () => {
    setMenu(undefined);
    const back = menuFrom;
    const name = menuFor;
    requestAnimationFrame(() => {
      if (back && back.isConnected && box.contains(back)) return back.focus({ preventScroll: true });
      // The tile moved (pinned / hidden): find it again, else the first tile.
      const el = Array.from(box.querySelectorAll<HTMLElement>(".gtile")).find((t) => t.getAttribute("data-group") === name) ?? box.querySelector<HTMLElement>(".gtile");
      if (el) focusElement(el, { smooth: false });
    });
  };

  return (
    <>
      <div ref={box} class="gbrowse" role="dialog" aria-modal="true" aria-label="All groups" data-nav-trap>
        <header class="gbrowse__head">
          <div class="gbrowse__titles">
            <p class="eyebrow eyebrow--accent">Live TV</p>
            <h2 class="gbrowse__title">All groups</h2>
            <p class="gbrowse__sub">
              {allGroups().length - groupPrefs().hidden.length} groups · {totalChannels().toLocaleString()} channels
            </p>
          </div>
          <label class="livesearch gbrowse__search">
            <Icon name="search" />
            <input
              ref={search}
              type="search"
              placeholder="Search groups — name, “sports”, “arabic”…"
              value={q()}
              onInput={(e) => {
                setQ(e.currentTarget.value);
                body.scrollTop = 0;
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  const first = sections()[0]?.items[0];
                  if (first) open(first);
                }
              }}
              aria-label="Search groups"
              autocomplete="off"
              spellcheck={false}
            />
            <Show when={q()}>
              <button type="button" class="livesearch__clear" aria-label="Clear" onClick={() => setQ("")}><Icon name="x" /></button>
            </Show>
          </label>
          <div class="gbrowse__quick">
            <button type="button" class="btn btn--sm" classList={{ "is-current": cur() === "favorites" }} onClick={() => props.onPick({ kind: "favorites" })}>
              <Icon name="star" /> <span>Favourites</span> <small>{favouriteCount()}</small>
            </button>
            <button type="button" class="btn btn--sm" classList={{ "is-current": cur() === "all" }} onClick={() => props.onPick({ kind: "all" })}>
              <Icon name="layers" /> <span>All channels</span>
            </button>
            <button type="button" class="iconbtn gbrowse__close" aria-label="Close" onClick={props.onClose}><Icon name="x" /></button>
          </div>
        </header>

        <div class="gbrowse__bar">
          <div class="segmented gbrowse__sort" role="group" aria-label="Arrange groups">
            <button type="button" class="segmented__btn" aria-pressed={sort() === "type"} onClick={() => setSort("type")}>By type</button>
            <button type="button" class="segmented__btn" aria-pressed={sort() === "az"} onClick={() => setSort("az")}>A–Z</button>
          </div>
          <Show when={!q()}>
            <div class="gbrowse__jump" data-nav-scroll="x" data-nav-group aria-label="Jump to">
              <For each={sections().filter((s) => s.items.length || s.hidden)}>
                {(s) => (
                  <button type="button" class="gbrowse__jumpbtn" classList={{ "is-letter": sort() === "az" && s.title.length === 1 }} onClick={() => jumpTo(s.id)}>
                    {s.id === "hidden" ? "Hidden" : s.title}
                  </button>
                )}
              </For>
            </div>
          </Show>
        </div>

        <div ref={body} class="gbrowse__body" data-nav-scroll="y">
          <For each={sectionIds()}>
            {(id) => {
              const s = () => sectionById().get(id) ?? { id, title: "", items: [] as GroupInfo[] };
              return (
              <section class="gsec" data-section={id} classList={{ "is-hidden": !!s().hidden }}>
                <Show when={s().title}>
                  <h3 class="gsec__title">
                    {s().title}
                    <Show when={!s().hidden && id !== "results"}><small>{s().items.length}</small></Show>
                  </h3>
                </Show>
                <Show when={s().hidden}>
                  <button type="button" class="btn btn--sm btn--quiet gsec__toggle" onClick={() => setShowHidden((v) => !v)}>
                    <Icon name={showHidden() ? "eyeoff" : "eye"} /> <span>{showHidden() ? "Hide the list" : "Show hidden groups"}</span>
                  </button>
                </Show>
                <div class="gsec__grid">
                  <For each={s().items} fallback={<Show when={id === "results"}><p class="gbrowse__none">No group matches “{q()}”.</p></Show>}>
                    {(g) => <GroupTile g={g} current={cur() === `g:${g.name}`} onOpen={() => (isHidden(g.name) ? openMenu(g) : open(g))} onMenu={() => openMenu(g)} />}
                  </For>
                </div>
              </section>
              );
            }}
          </For>
        </div>
      </div>

      <Show when={menu()}>{(g) => <GroupMenu g={g()} onOpen={() => { closeMenu(); open(g()); }} onClose={closeMenu} />}</Show>
    </>
  );
}

function GroupTile(props: { g: GroupInfo; current: boolean; onOpen: () => void; onMenu: () => void }): JSX.Element {
  const g = () => props.g;
  const logos = () => groupLogos(g().name, 4);
  const pinned = () => groupPrefs().pinned.indexOf(g().name) >= 0;
  const hidden = () => isHidden(g().name);
  const h = pressHandlers(() => props.onOpen(), () => props.onMenu());
  const initials = () => {
    const w = shortName(g().name).replace(/[^\p{L}\p{N} ]/gu, " ").split(/\s+/).filter(Boolean);
    return (w.length > 1 ? w[0]!.charAt(0) + w[1]!.charAt(0) : (w[0] ?? "?").slice(0, 2)).toUpperCase();
  };
  return (
    <div class="gtile-wrap" classList={{ "is-hidden": hidden() }}>
      <button
        type="button"
        class="gtile"
        data-group={g().name}
        aria-current={props.current ? "true" : undefined}
        aria-label={`${g().name}, ${g().count} channels${pinned() ? ", pinned" : ""}${hidden() ? ", hidden" : ""}`}
        style={{ "--tint": tint(g()) }}
        {...h}
      >
        <span class="gtile__art" classList={{ "has-logos": logos().length >= 2 }}>
          <Show when={logos().length >= 2} fallback={<span class="gtile__mono">{initials()}</span>}>
            <For each={logos()}>
              {(c) => (
                <span class="gtile__logo">
                  <Img src={mediaUrl(c.logo_url)} alt="" />
                </span>
              )}
            </For>
          </Show>
        </span>
        <span class="gtile__body">
          <span class="gtile__name" dir="auto">{g().name}</span>
          <span class="gtile__meta">
            <span>{g().count.toLocaleString()} ch</span>
            <Show when={g().topic || g().region}><span class="gtile__tag">{[g().topic?.label, g().region?.label].filter(Boolean).join(" · ")}</span></Show>
          </span>
        </span>
        <Show when={pinned()}><span class="gtile__pin" title="Pinned"><Icon name="star" /></span></Show>
        <Show when={props.current}><span class="gtile__now">Selected</span></Show>
      </button>
      <button type="button" class="gtile__more" tabindex="-1" data-nav-skip aria-label={`Options for ${g().name}`} onClick={() => props.onMenu()}>
        <span aria-hidden="true">⋯</span>
      </button>
    </div>
  );
}

/** Pin / hide / open — a small modal so it works the same with a remote and a mouse. */
export function GroupMenu(props: { g: GroupInfo; onOpen: () => void; onClose: () => void }): JSX.Element {
  let box!: HTMLDivElement;
  const pinned = () => groupPrefs().pinned.indexOf(props.g.name) >= 0;
  const hidden = () => isHidden(props.g.name);
  onMount(() => {
    const off = onBack(() => {
      props.onClose();
      return true;
    });
    onCleanup(off);
    requestAnimationFrame(() => box.querySelector<HTMLElement>("[data-autofocus]")?.focus({ preventScroll: true }));
  });
  return (
    <>
      <div class="gmenu__scrim" onClick={props.onClose} aria-hidden="true" />
      <div ref={box} class="gmenu" role="dialog" aria-modal="true" aria-label={props.g.name} data-nav-trap>
        <p class="gmenu__name" dir="auto">{props.g.name}</p>
        <p class="gmenu__sub">{props.g.count.toLocaleString()} channels{props.g.topic ? ` · ${props.g.topic.label}` : ""}{props.g.region ? ` · ${props.g.region.label}` : ""}</p>
        <div class="gmenu__list">
          <Show when={!hidden()}>
            <button type="button" class="gmenu__item" data-autofocus onClick={props.onOpen}><Icon name="play" /> <span>Open group</span></button>
            <button type="button" class="gmenu__item" onClick={() => { pinGroup(props.g.name, !pinned()); props.onClose(); }}>
              <Icon name="star" /> <span>{pinned() ? "Unpin from Live TV" : "Pin to Live TV"}</span>
            </button>
          </Show>
          <button type="button" class="gmenu__item" data-autofocus={hidden() ? "" : undefined} onClick={() => { hideGroup(props.g.name, !hidden()); props.onClose(); }}>
            <Icon name={hidden() ? "eye" : "eyeoff"} /> <span>{hidden() ? "Show this group again" : "Hide this group"}</span>
          </button>
          <button type="button" class="gmenu__item gmenu__item--quiet" onClick={props.onClose}><Icon name="x" /> <span>Cancel</span></button>
        </div>
      </div>
    </>
  );
}

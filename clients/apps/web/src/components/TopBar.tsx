import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { MOCK, TV, href } from "../lib/mode";
import { AccountMenu, TopMenu } from "./AccountMenu";

export interface TopBarLink {
  path: string;
  label: string;
  current: boolean;
  /** Small count after the label (Requests). */
  count?: number;
}

export interface TopBarProps {
  links: TopBarLink[];
  /** Only the Home billboard runs under a see-through bar; every other page has the glass on. */
  overHero: boolean;
  searchCurrent: boolean;
  downloadsCurrent: boolean;
  downloads: { active?: { progress: number; state: string }; count: number; failed: number };
  username?: string;
  settingsPath: string;
  onSignOut: () => void;
}

/** The main navigation on desktop and TV. Phones use the bottom bar instead (CSS hides this one). */
export function TopBar(props: TopBarProps): JSX.Element {
  const [scrolled, setScrolled] = createSignal(false);
  onMount(() => {
    const onScroll = () => setScrolled(window.pageYOffset > 8);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => window.removeEventListener("scroll", onScroll));
  });

  const dlBadge = () => props.downloads.count || (props.downloads.failed > 0 ? "!" : "");
  const currentLabel = () => props.links.find((l) => l.current)?.label ?? "Browse";

  return (
    <header
      class="topbar"
      classList={{ "is-solid": !props.overHero || scrolled() }}
      data-nav-group
      data-nav-fixed
      data-nav-top
    >
      <a class="topbar__brand" href={href("/")} tabindex="-1" data-nav-skip aria-label="Reel home">
        <span class="logo">
          <span class="logo__mark" aria-hidden="true"><i /></span>
          <span class="logo__word">reel</span>
        </span>
      </a>

      <nav class="topbar__links" aria-label="Main">
        <For each={props.links}>
          {(l) => (
            <a class="topbar__link" href={href(l.path)} aria-current={l.current ? "page" : undefined}>
              {l.label}
              <Show when={l.count}>
                <span class="topbar__pill" aria-label={`${l.count} active`}>{l.count}</span>
              </Show>
            </a>
          )}
        </For>
      </nav>

      {/* Narrow desktop windows only: the same links behind one button. Never on TV. */}
      <Show when={!TV}>
        <TopMenu
          label="Browse"
          class="topbar__browse"
          buttonClass="topbar__link topbar__browse-btn"
          align="left"
          button={
            <>
              <span>{currentLabel()}</span>
              <span class="topbar__caret" aria-hidden="true" />
            </>
          }
        >
          {(close) => (
            <For each={props.links}>
              {(l) => (
                <a
                  class="topmenu__item"
                  classList={{ "is-current": l.current }}
                  role="menuitem"
                  href={href(l.path)}
                  aria-current={l.current ? "page" : undefined}
                  onClick={close}
                >
                  <span>{l.label}</span>
                  <Show when={l.count}>
                    <span class="topbar__pill">{l.count}</span>
                  </Show>
                </a>
              )}
            </For>
          )}
        </TopMenu>
      </Show>

      <div class="topbar__tools">
        <Show when={MOCK}>
          <span class="topbar__tag" title="Showing built-in sample data (?mock=0 to turn off)">Sample data</span>
        </Show>
        <a
          class="topbar__tool"
          href={href("/search")}
          aria-label="Search"
          aria-current={props.searchCurrent ? "page" : undefined}
          aria-keyshortcuts={TV ? undefined : "/ Control+K Meta+K"}
        >
          <Icon name="search" />
        </a>
        <a
          class="topbar__tool"
          href={href("/downloads")}
          aria-label={props.downloads.count ? `Downloads, ${props.downloads.count} active` : "Downloads"}
          aria-current={props.downloadsCurrent ? "page" : undefined}
        >
          <Icon name="download" />
          <Show when={props.downloads.active}>
            {(a) => <MiniRing progress={a().progress} state={a().state} />}
          </Show>
          <Show when={dlBadge()}>
            <span class="topbar__count" classList={{ "topbar__count--bad": dlBadge() === "!" }} aria-hidden="true">
              {dlBadge()}
            </span>
          </Show>
        </a>
        <AccountMenu username={props.username} settingsPath={props.settingsPath} onSignOut={props.onSignOut} />
      </div>
    </header>
  );
}

/** Aggregate download progress drawn around the Downloads icon. */
function MiniRing(props: { progress: number; state: string }): JSX.Element {
  const C = 2 * Math.PI * 21;
  return (
    <svg class={`topbar__ring topbar__ring--${props.state}`} viewBox="0 0 48 48" aria-hidden="true">
      <circle cx="24" cy="24" r="21" class="topbar__ring-track" />
      <circle
        cx="24"
        cy="24"
        r="21"
        class="topbar__ring-arc"
        stroke-dasharray={`${C} ${C}`}
        stroke-dashoffset={C * (1 - props.progress)}
        transform="rotate(-90 24 24)"
      />
    </svg>
  );
}

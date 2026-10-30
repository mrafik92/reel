import { For, Show, createSignal, onCleanup, onMount, type JSX, type ParentProps } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Icon, initTvNavigation, onBack, type IconName } from "@reel/ui";
import { MOCK, TV, href } from "../lib/mode";

interface NavItem {
  path: string;
  label: string;
  icon: IconName;
  match: (p: string) => boolean;
}

const NAV: NavItem[] = [
  { path: "/", label: "Home", icon: "home", match: (p) => p === "/" },
  { path: "/movies", label: "Movies", icon: "film", match: (p) => p.indexOf("/movies") === 0 },
  { path: "/shows", label: "Shows", icon: "tv", match: (p) => p.indexOf("/shows") === 0 },
  { path: "/search", label: "Search", icon: "search", match: (p) => p.indexOf("/search") === 0 },
  // Phase 5: { path: "/requests", label: "Requests", icon: "inbox", ... }
  // Phase 6: { path: "/live", label: "Live TV", icon: "live", ... }
];

export function Shell(props: ParentProps): JSX.Element {
  const loc = useLocation();
  const navigate = useNavigate();
  const immersive = () => loc.pathname.indexOf("/play/") === 0;
  const [railOpen, setRailOpen] = createSignal(false);

  initTvNavigation({ tv: TV });

  onMount(() => {
    // Default Back: up one level; on Home, let the platform handle exit.
    const off = onBack(() => {
      if (loc.pathname === "/") return false;
      if (window.history.length > 1) window.history.back();
      else navigate("/");
      return true;
    });
    onCleanup(off);
  });

  return (
    <div class="app" classList={{ "is-immersive": immersive() }}>
      <Show when={!immersive()}>
        <nav
          class="rail"
          classList={{ "is-open": railOpen() }}
          aria-label="Main"
          data-nav-group
          data-nav-fixed
          onFocusIn={() => setRailOpen(true)}
          onFocusOut={(e) => {
            const next = e.relatedTarget as Node | null;
            if (!next || !(e.currentTarget as HTMLElement).contains(next)) setRailOpen(false);
          }}
        >
          <div class="rail__bg" aria-hidden="true" />
          <a class="rail__brand" href={href("/")} tabindex="-1" data-nav-skip aria-label="Reel home">
            <span class="logo">
              <span class="logo__mark" aria-hidden="true">
                <i />
              </span>
              <span class="logo__word">reel</span>
            </span>
          </a>
          <ul class="rail__list">
            <For each={NAV}>
              {(n) => (
                <li>
                  <a
                    class="rail__link"
                    href={href(n.path)}
                    aria-current={n.match(loc.pathname) ? "page" : undefined}
                  >
                    <Icon name={n.icon} class="rail__icon" />
                    <span class="rail__label">{n.label}</span>
                  </a>
                </li>
              )}
            </For>
          </ul>
          <div class="rail__foot">
            <Show when={MOCK || import.meta.env.DEV}>
              <a
                class="rail__link rail__link--quiet"
                href={href("/dev/rings")}
                aria-current={loc.pathname.indexOf("/dev") === 0 ? "page" : undefined}
              >
                <Icon name="layers" class="rail__icon" />
                <span class="rail__label">Design lab</span>
              </a>
            </Show>
            <Show when={MOCK}>
              <span class="rail__badge" title="Showing built-in sample data (?mock=0 to turn off)">
                Sample data
              </span>
            </Show>
          </div>
        </nav>
      </Show>
      <main class="main" data-nav-group>
        {props.children}
      </main>
      <div class="grain" aria-hidden="true" />
    </div>
  );
}

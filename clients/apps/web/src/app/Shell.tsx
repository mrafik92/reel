import { For, Match, Show, Switch, createEffect, createSignal, onCleanup, onMount, type JSX, type ParentProps } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Icon, initTvNavigation, isFileOrigin, onBack, type IconName } from "@reel/ui";
import { MOCK, TV, href } from "../lib/mode";
import { authStatus, bootError, bootstrap, currentUser, isAdmin, logout, onAuthChange } from "../lib/auth";
import { connectEvents, disconnectEvents, downloads, queueSummary } from "../lib/downloads";
import { Toaster } from "../components/Toaster";
import { ErrorState } from "../components/States";
import "../styles/browse.css";

interface NavItem {
  path: string;
  label: string;
  icon: IconName;
  match: (p: string) => boolean;
  admin?: boolean;
  /** Reserved slot: not shown until its section ships. */
  hidden?: boolean;
}

const NAV: NavItem[] = [
  // Search leads the rail (accent-ringed) so it's the first thing the D-pad meets.
  { path: "/search", label: "Search", icon: "search", match: (p) => p.indexOf("/search") === 0 },
  { path: "/", label: "Home", icon: "home", match: (p) => p === "/" },
  { path: "/movies", label: "Movies", icon: "film", match: (p) => p.indexOf("/movies") === 0 },
  { path: "/shows", label: "Shows", icon: "tv", match: (p) => p.indexOf("/shows") === 0 },
  { path: "/discover", label: "Discover", icon: "compass", match: (p) => p.indexOf("/discover") === 0 },
  { path: "/requests", label: "Requests", icon: "inbox", match: (p) => p.indexOf("/requests") === 0 },
  { path: "/downloads", label: "Downloads", icon: "download", match: (p) => p.indexOf("/downloads") === 0 },
  // Next: Live TV (IPTV guide + zapping). Flip `hidden` when the route lands.
  { path: "/live", label: "Live TV", icon: "live", match: (p) => p.indexOf("/live") === 0, hidden: true },
];

/** Routes reachable without a session. */
const PUBLIC = /^\/(login|setup|invite\/|connect)/;

export function Shell(props: ParentProps): JSX.Element {
  const loc = useLocation();
  const navigate = useNavigate();
  const immersive = () => loc.pathname.indexOf("/play/") === 0;
  const isPublic = () => PUBLIC.test(loc.pathname);
  const [railOpen, setRailOpen] = createSignal(false);

  initTvNavigation({ tv: TV });

  onMount(() => {
    void bootstrap();
    const offAuth = onAuthChange((u) => (u ? connectEvents() : disconnectEvents()));
    // Default Back: up one level; on Home, let the platform handle exit.
    const off = onBack(() => {
      if (loc.pathname === "/") return false;
      if (window.history.length > 1) window.history.back();
      else navigate("/");
      return true;
    });
    onCleanup(() => {
      off();
      offAuth();
      disconnectEvents();
    });
  });

  // Auth guard: send anonymous visitors to setup/login, signed-in ones away from them.
  createEffect(() => {
    const s = authStatus();
    const path = loc.pathname;
    if (s === "needs_server" && path !== "/connect") navigate("/connect", { replace: true });
    else if (s === "needs_setup" && path !== "/setup" && path.indexOf("/invite/") !== 0) navigate("/setup", { replace: true });
    else if (s === "anon" && !isPublic()) navigate(`/login${path !== "/" ? `?next=${encodeURIComponent(path + loc.search)}` : ""}`, { replace: true });
    else if (s === "authed" && /^\/(login|setup)/.test(path)) navigate("/", { replace: true });
  });

  // Global search shortcuts: "/" (when not typing) and Ctrl/Cmd+K (always).
  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  // The floating search pill tucks away once the page scrolls under it.
  const [tucked, setTucked] = createSignal(false);
  const openSearch = () => {
    if (loc.pathname.indexOf("/search") === 0) {
      const input = document.querySelector<HTMLInputElement>(".search__input");
      if (input) {
        input.focus();
        input.select();
        window.scrollTo(0, 0);
      }
    } else navigate("/search");
  };
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (authStatus() !== "authed" || immersive() || isPublic() || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
      const cmdK = (e.metaKey || e.ctrlKey) && !e.altKey && (e.key === "k" || e.key === "K");
      const slash = e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey;
      if (!cmdK && !slash) return;
      // Leave the shortcut alone while a modal (sheet, picker) owns the screen.
      if (!cmdK && document.querySelector("[data-nav-trap]")) return;
      e.preventDefault();
      openSearch();
    };
    const onScroll = () => setTucked(window.pageYOffset > 90);
    document.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => {
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll);
    });
  });

  // Requests badge: active downloads of this user.
  const activeCount = () => {
    let n = 0;
    const all = downloads.byTmdb;
    for (const k in all) {
      const st = all[k]!.state;
      if (st !== "available" && st !== "declined") n++;
    }
    return n;
  };

  return (
    <div class="app" classList={{ "is-immersive": immersive() || isPublic() }}>
      <Switch
        fallback={
          <main class="main" data-nav-group>
            {props.children}
          </main>
        }
      >
        <Match when={isPublic()}>
          <main class="main main--bare">{props.children}</main>
        </Match>
        <Match when={authStatus() === "checking"}>
          <div class="splash" aria-busy="true" aria-label="Loading">
            <span class="logo logo--splash">
              <span class="logo__mark" aria-hidden="true"><i /></span>
              <span class="logo__word">reel</span>
            </span>
          </div>
        </Match>
        <Match when={authStatus() === "offline"}>
          <main class="main main--bare">
            <div class="page page--center">
              <ErrorState error={bootError()} onRetry={() => void bootstrap()} />
              <Show when={isFileOrigin()}>
                <a class="btn" href={href("/connect")}>
                  <Icon name="server" /> <span>Change server</span>
                </a>
              </Show>
            </div>
          </main>
        </Match>
        <Match when={authStatus() !== "authed"}>
          <div class="splash" />
        </Match>
        <Match when={immersive()}>
          <main class="main" data-nav-group>
            {props.children}
          </main>
        </Match>
        <Match when={authStatus() === "authed"}>
          <nav
            class="rail"
            classList={{ "is-open": railOpen() }}
            aria-label="Main"
            data-nav-group
            data-nav-lateral
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
              <For each={NAV.filter((n) => !n.hidden)}>
                {(n) => (
                  <li>
                    <a
                      class="rail__link"
                      classList={{ "rail__link--search": n.path === "/search" }}
                      href={href(n.path)}
                      aria-current={n.match(loc.pathname) ? "page" : undefined}
                      aria-keyshortcuts={n.path === "/search" && !TV ? "/ Control+K Meta+K" : undefined}
                    >
                      <Icon name={n.icon} class="rail__icon" />
                      <Show when={n.path === "/downloads" && queueSummary().active}>
                        {(a) => <MiniRing progress={a().progress} state={a().state} />}
                      </Show>
                      <span class="rail__label">
                        {n.label}
                        <Show when={n.path === "/downloads" && queueSummary().active}>
                          {(a) => <small class="rail__sub">{Math.floor(a().progress * 100)}% · {queueSummary().count} active</small>}
                        </Show>
                      </span>
                      <Show when={n.path === "/requests" && activeCount()}>
                        <span class="rail__count" aria-label={`${activeCount()} active`}>{activeCount()}</span>
                      </Show>
                      <Show when={n.path === "/downloads" && queueSummary().count + queueSummary().failed}>
                        <span class="rail__count" classList={{ "rail__count--bad": !queueSummary().count && queueSummary().failed > 0 }} aria-label={`${queueSummary().count} downloading`}>
                          {queueSummary().count || "!"}
                        </span>
                      </Show>
                    </a>
                  </li>
                )}
              </For>
            </ul>
            <div class="rail__foot">
              <Show when={isAdmin()}>
                <a class="rail__link" href={href("/admin")} aria-current={loc.pathname.indexOf("/admin") === 0 ? "page" : undefined}>
                  <Icon name="gear" class="rail__icon" />
                  <span class="rail__label">Settings</span>
                </a>
              </Show>
              <Show when={MOCK || import.meta.env.DEV}>
                <a class="rail__link rail__link--quiet" href={href("/dev/rings")} aria-current={loc.pathname.indexOf("/dev") === 0 ? "page" : undefined}>
                  <Icon name="layers" class="rail__icon" />
                  <span class="rail__label">Design lab</span>
                </a>
              </Show>
              <button
                type="button"
                class="rail__link rail__link--quiet"
                onClick={() => void logout().then(() => navigate("/login"))}
                title={currentUser() ? `Signed in as ${currentUser()!.username}` : undefined}
              >
                <Icon name="logout" class="rail__icon" />
                <span class="rail__label">Sign out{currentUser() ? ` (${currentUser()!.username})` : ""}</span>
              </button>
              <Show when={MOCK}>
                <span class="rail__badge" title="Showing built-in sample data (?mock=0 to turn off)">Sample data</span>
              </Show>
            </div>
          </nav>
          <Show when={loc.pathname.indexOf("/search") !== 0}>
            <a
              class="topsearch"
              classList={{ "is-tucked": tucked() }}
              href={href("/search")}
              data-nav-fixed
              data-nav-skip={tucked() ? "" : undefined}
              tabindex={tucked() ? -1 : undefined}
              aria-keyshortcuts={TV ? undefined : "/ Control+K Meta+K"}
            >
              <Icon name="search" class="topsearch__icon" />
              <span class="topsearch__label">Search</span>
              <Show when={!TV}>
                <span class="topsearch__keys" aria-hidden="true">
                  <span class="keycap">{isMac ? "⌘" : "Ctrl"}</span>
                  <span class="keycap">K</span>
                </span>
              </Show>
            </a>
          </Show>
          <main class="main" data-nav-group>
            {props.children}
          </main>
        </Match>
      </Switch>
      <Toaster />
      <div class="grain" aria-hidden="true" />
    </div>
  );
}

/** Aggregate download progress drawn around the nav icon. */
function MiniRing(props: { progress: number; state: string }): JSX.Element {
  const C = 2 * Math.PI * 21;
  return (
    <svg class={`rail__ring rail__ring--${props.state}`} viewBox="0 0 48 48" aria-hidden="true">
      <circle cx="24" cy="24" r="21" class="rail__ring-track" />
      <circle cx="24" cy="24" r="21" class="rail__ring-arc" stroke-dasharray={`${C} ${C}`} stroke-dashoffset={C * (1 - props.progress)} transform="rotate(-90 24 24)" />
    </svg>
  );
}

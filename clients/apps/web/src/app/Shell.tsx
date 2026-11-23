import { For, Match, Show, Switch, createEffect, createSignal, onCleanup, onMount, type JSX, type ParentProps } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Icon, Sheet, focusElement, initTvNavigation, isFileOrigin, onBack, type IconName, type SheetOption } from "@reel/ui";
import { MOCK, TV, href } from "../lib/mode";
import { authStatus, bootError, bootstrap, currentUser, isAdmin, logout, onAuthChange } from "../lib/auth";
import { connectEvents, disconnectEvents, downloads, queueSummary } from "../lib/downloads";
import { Toaster } from "../components/Toaster";
import { TopBar } from "../components/TopBar";
import { api, invalidate } from "../api";
import { hadNetworkFailure, onConnectionRestored } from "../api/http";
import { onResume } from "../lib/connection";
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
  /** Phone bottom bar (≤760px): lives behind the More tab instead of in the bar (#180). */
  more?: boolean;
}

const NAV: NavItem[] = [
  // The phone bottom bar's order; the top bar lists TOPBAR_PATHS instead.
  { path: "/search", label: "Search", icon: "search", match: (p) => p.indexOf("/search") === 0 },
  { path: "/", label: "Home", icon: "home", match: (p) => p === "/" },
  { path: "/movies", label: "Movies", icon: "film", match: (p) => p.indexOf("/movies") === 0 },
  { path: "/shows", label: "Shows", icon: "tv", match: (p) => p.indexOf("/shows") === 0 },
  { path: "/discover", label: "Discover", icon: "compass", match: (p) => p.indexOf("/discover") === 0, more: true },
  { path: "/live", label: "Live TV", icon: "live", match: (p) => p.indexOf("/live") === 0 },
  { path: "/requests", label: "Requests", icon: "inbox", match: (p) => p.indexOf("/requests") === 0, more: true },
  { path: "/downloads", label: "Downloads", icon: "download", match: (p) => p.indexOf("/downloads") === 0, more: true },
];

/** The top bar's primary links, in order. Search and Downloads are icons in its right cluster. */
const TOPBAR_PATHS = ["/", "/shows", "/movies", "/live", "/discover", "/requests"];

/** Pages behind the phone bar's More tab that aren't NAV items (Settings, the design lab). */
const MORE_EXTRA = /^\/(settings|admin|dev)(\/|$)/;
const LOGOUT = "logout";

/** Routes reachable without a session. */
const PUBLIC = /^\/(login|setup|invite\/|connect)/;

/** The pages in NAV, except Live TV, which keeps its own Back. */
function isTopLevel(path: string): boolean {
  return path.indexOf("/live") !== 0 && NAV.some((n) => n.match(path));
}

export function Shell(props: ParentProps): JSX.Element {
  const loc = useLocation();
  const navigate = useNavigate();
  const immersive = () => loc.pathname.indexOf("/play/") === 0;
  const isPublic = () => PUBLIC.test(loc.pathname);
  // Phone bottom bar: the destinations that don't fit sit in a sheet behind "More" (#180).
  const [moreOpen, setMoreOpen] = createSignal(false);

  initTvNavigation({ tv: TV });

  // #81: a page whose reads failed while the network was away (a blip, a frozen background tab, a
  // reset HTTP/3 connection) is loaded again as soon as the server answers — no refresh needed. The
  // player and Live TV recover their streams themselves and are never re-mounted.
  const [pageEpoch, setPageEpoch] = createSignal(1);
  const reloadPage = () => {
    if (immersive() || loc.pathname.indexOf("/live") === 0 || authStatus() !== "authed") return;
    invalidate();
    setPageEpoch((n) => n + 1);
  };
  onMount(() => {
    const offRestored = onConnectionRestored(reloadPage);
    // Back in the tab / online again: probe once, which fires `restored` if the server answers.
    const offResume = onResume(() => {
      if (hadNetworkFailure() && authStatus() === "authed") api.me().catch(() => undefined);
    });
    onCleanup(() => {
      offRestored();
      offResume();
    });
  });

  onMount(() => {
    void bootstrap();
    const offAuth = onAuthChange((u) => (u ? connectEvents() : disconnectEvents()));
    // Default Back: up one level. On Home, and on sign-in/setup while signed out (where "up" only
    // redirects back to the same page), let the platform handle it: TVs and Reel Web for Android
    // leave the app (#227).
    const off = onBack(() => {
      // TV: with no rail to reach sideways, Back on a scrolled top-level page returns to the top bar.
      if (TV && window.pageYOffset > 0 && isTopLevel(loc.pathname)) {
        window.scrollTo(0, 0);
        const link = document.querySelector<HTMLElement>('.topbar [aria-current="page"]') ?? document.querySelector<HTMLElement>(".topbar__link");
        if (link) focusElement(link, { smooth: false });
        return true;
      }
      if (loc.pathname === "/" || (isPublic() && authStatus() !== "authed")) return false;
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
    document.addEventListener("keydown", onKey);
    onCleanup(() => document.removeEventListener("keydown", onKey));
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

  const settingsPath = () => (isAdmin() ? "/admin" : "/settings/playback");
  const inMore = () => MORE_EXTRA.test(loc.pathname) || NAV.some((n) => n.more && !n.hidden && n.match(loc.pathname));
  const moreOptions = (): SheetOption<string>[] => {
    const q = queueSummary();
    const reqs = activeCount();
    const opts: SheetOption<string>[] = NAV.filter((n) => n.more && !n.hidden).map((n) => ({
      value: n.path,
      title: n.label,
      icon: n.icon,
      detail:
        n.path === "/downloads" && q.active
          ? `${Math.floor(q.active.progress * 100)}% · ${q.count} active`
          : n.path === "/downloads" && q.failed
            ? `${q.failed} failed`
            : n.path === "/requests" && reqs
              ? `${reqs} active`
              : undefined,
    }));
    opts.push({ value: settingsPath(), title: "Settings", icon: "gear" });
    if (MOCK || import.meta.env.DEV) opts.push({ value: "/dev/rings", title: "Design lab", icon: "layers" });
    opts.push({ value: LOGOUT, title: "Sign out", icon: "logout" });
    return opts;
  };
  const moreSelected = () => {
    const p = loc.pathname;
    const n = NAV.find((x) => x.more && x.match(p));
    if (n) return n.path;
    if (/^\/(settings|admin)(\/|$)/.test(p)) return settingsPath();
    if (p.indexOf("/dev") === 0) return "/dev/rings";
    return "";
  };
  // Same signal as the Downloads tab it stands in for: active downloads, or "!" when only failures.
  const moreBadge = () => queueSummary().count || (queueSummary().failed > 0 ? "!" : "");
  const signOut = () => void logout().then(() => navigate("/login"));
  const topLinks = () =>
    TOPBAR_PATHS.map((p) => NAV.find((n) => n.path === p)!)
      .filter((n) => !n.hidden)
      .map((n) => ({
        path: n.path,
        label: n.label,
        current: n.match(loc.pathname),
        count: n.path === "/requests" ? activeCount() : undefined,
      }));

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
          <TopBar
            links={topLinks()}
            overHero={loc.pathname === "/"}
            searchCurrent={loc.pathname.indexOf("/search") === 0}
            downloadsCurrent={loc.pathname.indexOf("/downloads") === 0}
            downloads={queueSummary()}
            username={currentUser()?.username}
            settingsPath={settingsPath()}
            onSignOut={signOut}
          />
          {/* The phone bottom bar. Width isn't a constant, so CSS picks it or the top bar; TV has only the top bar. */}
          <Show when={!TV}>
            <nav class="rail" aria-label="Main" data-nav-group data-nav-lateral data-nav-fixed>
              <ul class="rail__list">
                <For each={NAV.filter((n) => !n.hidden && !n.more)}>
                  {(n) => (
                    <li>
                      <a
                        class="rail__link"
                        href={href(n.path)}
                        aria-current={n.match(loc.pathname) ? "page" : undefined}
                        aria-keyshortcuts={n.path === "/search" ? "/ Control+K Meta+K" : undefined}
                      >
                        <Icon name={n.icon} class="rail__icon" />
                        <span class="rail__label">{n.label}</span>
                      </a>
                    </li>
                  )}
                </For>
                <li>
                  <button
                    type="button"
                    class="rail__link"
                    aria-haspopup="dialog"
                    aria-expanded={moreOpen() ? "true" : "false"}
                    aria-current={inMore() ? "true" : undefined}
                    onClick={() => setMoreOpen(true)}
                  >
                    <Icon name="more" class="rail__icon" />
                    <span class="rail__label">More</span>
                    <Show when={moreBadge()}>
                      <span class="rail__count" classList={{ "rail__count--bad": moreBadge() === "!" }} aria-hidden="true">
                        {moreBadge()}
                      </span>
                    </Show>
                  </button>
                </li>
              </ul>
            </nav>
          </Show>
          <Show when={moreOpen()}>
            <Sheet
              title="More"
              options={moreOptions()}
              selected={moreSelected()}
              onClose={() => setMoreOpen(false)}
              onSelect={(v) => {
                if (v === LOGOUT) signOut();
                else navigate(v);
              }}
              footer={<>Signed in as {currentUser()?.username ?? "…"}</>}
            />
          </Show>
          <main class="main" data-nav-group>
            {/* Re-mounted when the connection comes back after a page's load failed (#81). */}
            <Show when={pageEpoch()} keyed>
              {(_n: number) => <>{props.children}</>}
            </Show>
          </main>
        </Match>
      </Switch>
      <Toaster />
    </div>
  );
}

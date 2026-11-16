import { createSignal, For, lazy, Match, onCleanup, Show, Switch, type JSX } from "solid-js";
import { useParams } from "@solidjs/router";
import { Icon, type IconName } from "@reel/ui";
import { href } from "../../lib/mode";
import { isAdmin } from "../../lib/auth";
import { usePageFocus } from "../../lib/focus";

const Libraries = lazy(() => import("./Libraries"));
const Users = lazy(() => import("./Users"));
const Arr = lazy(() => import("./Arr"));
const DownloadClients = lazy(() => import("./DownloadClients"));
const Notifications = lazy(() => import("./Notifications"));
const Sessions = lazy(() => import("./Sessions"));
const Hardware = lazy(() => import("./Hardware"));
const Iptv = lazy(() => import("./Iptv"));
const Playback = lazy(() => import("../SettingsPlayback"));
import About from "./About";

const SECTIONS: { id: string; label: string; icon: IconName; blurb: string }[] = [
  { id: "libraries", label: "Libraries", icon: "folder", blurb: "Folders Reel scans" },
  { id: "users", label: "Users & invites", icon: "user", blurb: "Who can watch, and how much" },
  { id: "arr", label: "Radarr & Sonarr", icon: "server", blurb: "Where requests go" },
  { id: "downloads", label: "Download clients", icon: "download", blurb: "Speed, peers, ETA" },
  { id: "notifications", label: "Notifications", icon: "bell", blurb: "Discord, Telegram, webhooks" },
  { id: "iptv", label: "Live TV", icon: "live", blurb: "IPTV providers, channels, guide" },
  { id: "sessions", label: "Now playing", icon: "activity", blurb: "Active streams" },
  { id: "hardware", label: "Transcoding", icon: "chip", blurb: "Hardware acceleration" },
];

/** Your own settings (every user): audio/subtitle choices and how Live TV plays on this device.
 * Lives at `/settings/playback` (also `/admin/playback`), inside this layout (#88). */
function PlaybackLink(props: { current: boolean }): JSX.Element {
  return (
    <a
      class="admin__link"
      href={href("/settings/playback")}
      aria-current={props.current ? "page" : undefined}
      data-autofocus={props.current ? "" : undefined}
    >
      <Icon name="play" />
      <span>
        <strong>Playback</strong>
        <small>Quality, audio, subtitles, Live TV</small>
      </span>
    </a>
  );
}

/**
 * Settings: one layout with the section nav on the left and the section in the pane.
 * Everyone gets their own sections (Playback, About); admins also get the server sections.
 * `section` comes from the route (`/settings/playback` passes it) or `/admin/:section`.
 */
export default function Admin(props: { section?: string }): JSX.Element {
  const params = useParams<{ section?: string }>();
  const section = () => props.section ?? params.section ?? (isAdmin() ? "libraries" : "playback");
  usePageFocus(() => true);

  // Beside the pane (wider than 960px, always on TV) the nav is a side rail: left/right hop between
  // it and the pane even when the pane's first control is lower down (Playback), and up/down stay
  // on their own side. Narrower, it's a strip above the pane and up/down must reach the pane.
  const wide = typeof matchMedia === "function" ? matchMedia("(min-width: 961px)") : null;
  const [beside, setBeside] = createSignal(wide ? wide.matches : true);
  if (wide) {
    const sync = () => setBeside(wide.matches);
    wide.addListener(sync); // addEventListener("change") needs Chromium 73+; webOS ships 68
    onCleanup(() => wide.removeListener(sync));
  }

  return (
    <div class="page admin">
      <header class="page-head">
        <h1 class="page-title">Settings</h1>
      </header>
      <div class="admin__layout">
        <nav class="admin__nav" aria-label="Settings sections" data-nav-group data-nav-lateral={beside() ? "" : undefined}>
          <PlaybackLink current={section() === "playback"} />
          <Show when={isAdmin()}>
            <p class="admin__navhead">Server</p>
            <For each={SECTIONS}>
              {(s) => (
                <a class="admin__link" href={href(`/admin/${s.id}`)} aria-current={section() === s.id ? "page" : undefined} data-autofocus={section() === s.id ? "" : undefined}>
                  <Icon name={s.icon} />
                  <span>
                    <strong>{s.label}</strong>
                    <small>{s.blurb}</small>
                  </span>
                </a>
              )}
            </For>
          </Show>
          <About />
        </nav>
        <div class="admin__content" classList={{ "admin__content--wide": section() === "playback" }} data-nav-group>
          <Switch fallback={<p>Unknown section.</p>}>
            <Match when={section() === "playback"}><Playback /></Match>
            <Match when={!isAdmin()}>
              <p class="state__body">Only admins can change server settings.</p>
            </Match>
            <Match when={section() === "libraries"}><Libraries /></Match>
            <Match when={section() === "users"}><Users /></Match>
            <Match when={section() === "arr"}><Arr /></Match>
            <Match when={section() === "downloads"}><DownloadClients /></Match>
            <Match when={section() === "notifications"}><Notifications /></Match>
            <Match when={section() === "sessions"}><Sessions /></Match>
            <Match when={section() === "hardware"}><Hardware /></Match>
            <Match when={section() === "iptv"}><Iptv /></Match>
          </Switch>
        </div>
      </div>
    </div>
  );
}

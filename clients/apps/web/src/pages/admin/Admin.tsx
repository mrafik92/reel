import { For, lazy, Match, Show, Switch, type JSX } from "solid-js";
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

/** Your own settings (every user): audio/subtitle choices and how Live TV plays on this device. */
function PlaybackLink(): JSX.Element {
  return (
    <a class="admin__link" href={href("/settings/playback")}>
      <Icon name="play" />
      <span>
        <strong>Playback</strong>
        <small>Audio, subtitles, Live TV</small>
      </span>
    </a>
  );
}

export default function Admin(): JSX.Element {
  const params = useParams<{ section?: string }>();
  const section = () => params.section ?? "libraries";
  usePageFocus(() => true);

  return (
    <div class="page admin">
      <header class="page-head">
        <p class="eyebrow eyebrow--accent">Settings</p>
        <h1 class="page-title">Server</h1>
      </header>
      <Show
        when={isAdmin()}
        fallback={
          <>
            <nav class="admin__nav" aria-label="Your settings" data-nav-group>
              <PlaybackLink />
            </nav>
            <p class="state__body">Only admins can change server settings.</p>
            <About />
          </>
        }
      >
        <div class="admin__layout">
          <nav class="admin__nav" aria-label="Settings sections" data-nav-group>
            <PlaybackLink />
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
            <About />
          </nav>
          <div class="admin__content" data-nav-group>
            <Switch fallback={<p>Unknown section.</p>}>
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
      </Show>
    </div>
  );
}

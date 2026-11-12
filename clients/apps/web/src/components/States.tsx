import { Show, createEffect, onCleanup, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { ApiError } from "../api";
import { MOCK, href } from "../lib/mode";
import { isAdmin } from "../lib/auth";
import { autoRetry } from "../lib/connection";

const RETRY_DELAYS = [3000, 6000, 12000, 20000, 30000];
let offlineStreak = 0;
let lastOfflineAt = 0;

export function ErrorState(props: { error: unknown; onRetry?: () => void; title?: string }): JSX.Element {
  const offline = () => props.error instanceof ApiError && props.error.status === 0;
  const notFound = () => props.error instanceof ApiError && props.error.status === 404;
  const message = () =>
    props.error instanceof Error ? props.error.message : "Something went wrong";
  // Couldn't reach the server (#81): retry by itself with backoff and when the tab comes back.
  const auto = () => offline() && !!props.onRetry;
  createEffect(() => {
    if (!auto()) return;
    // A failed retry often remounts this card (loading → error): keep backing off across mounts.
    const now = Date.now();
    offlineStreak = now - lastOfflineAt < 120_000 ? offlineStreak + 1 : 0;
    lastOfflineAt = now;
    const delays = RETRY_DELAYS.slice(Math.min(offlineStreak, RETRY_DELAYS.length - 1));
    const stop = autoRetry(() => props.onRetry?.(), auto, delays);
    onCleanup(stop);
  });
  return (
    <div class="state state--error" role="alert">
      <div class="state__icon">
        <Icon name="alert" />
      </div>
      <h2 class="state__title">
        {props.title ?? (offline() ? "Can't reach your Reel server" : notFound() ? "This title isn't here anymore" : "Something went wrong")}
      </h2>
      <p class="state__body">
        <Show when={offline()} fallback={message()}>
          Check that the server is running and on the same network.
          <Show when={props.onRetry}> Reel keeps trying in the background.</Show>
          <Show when={!MOCK}>
            {" "}To preview the interface with sample data, add <code>?mock=1</code> to the address.
          </Show>
        </Show>
      </p>
      <Show when={props.onRetry}>
        <button class="btn btn--primary" type="button" data-autofocus onClick={() => props.onRetry?.()}>
          <Icon name="retry" /> <span>Try again</span>
        </button>
      </Show>
    </div>
  );
}

export function EmptyState(props: { title: string; body?: string; children?: JSX.Element }): JSX.Element {
  return (
    <div class="state">
      <h2 class="state__title">{props.title}</h2>
      <Show when={props.body}>
        <p class="state__body">{props.body}</p>
      </Show>
      {props.children}
    </div>
  );
}

/** Skeleton row for loading states. */
export function SkeletonRow(props: { variant?: "poster" | "landscape"; count?: number; title?: boolean }): JSX.Element {
  const n = props.count ?? 8;
  return (
    <section class={`row row--${props.variant ?? "poster"} is-skeleton`} aria-hidden="true">
      <Show when={props.title !== false}>
        <header class="row__head">
          <span class="skel skel--text" style={{ width: "12rem" }} />
        </header>
      </Show>
      <div class="row__scroller">
        <div class="row__track">
          {Array.from({ length: n }, () => (
            <div class={`card card--${props.variant ?? "poster"}`}>
              <div class="card__frame skel" />
              <div class="card__caption">
                <span class="skel skel--text" style={{ width: "70%" }} />
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

/** No libraries yet: admins get a clear next step, everyone else a friendly note. */
export function FirstRun(): JSX.Element {
  return (
    <div class="state state--first">
      <p class="eyebrow eyebrow--accent">Welcome to Reel</p>
      <h2 class="state__title">{isAdmin() ? "Add your first library" : "Nothing to watch yet"}</h2>
      <p class="state__body">
        {isAdmin()
          ? "Point Reel at the folders holding your films and series. It scans them, matches each title on TMDB and fetches artwork."
          : "Your server doesn't have any libraries you can see yet. Ask the person who invited you to share one."}
      </p>
      <Show when={isAdmin()}>
        <div class="actions">
          <a class="btn btn--primary btn--lg" href={href("/admin/libraries")} data-autofocus>
            <Icon name="plus" /> <span>Add a library</span>
          </a>
          <a class="btn btn--lg" href={href("/discover")}>
            <Icon name="compass" /> <span>Browse Discover</span>
          </a>
        </div>
      </Show>
    </div>
  );
}

import { Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { ApiError } from "../api";
import { MOCK } from "../lib/mode";

export function ErrorState(props: { error: unknown; onRetry?: () => void; title?: string }): JSX.Element {
  const offline = () => props.error instanceof ApiError && props.error.status === 0;
  const notFound = () => props.error instanceof ApiError && props.error.status === 404;
  const message = () =>
    props.error instanceof Error ? props.error.message : "Something went wrong";
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

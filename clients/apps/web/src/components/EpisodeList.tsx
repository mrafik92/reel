import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { formatRuntime, Icon, Img, type ItemSummary } from "@reel/ui";
import { api } from "../api";
import { playHref } from "../lib/links";

/** Children are summaries without an overview: fetch synopses as rows appear, a few at a time. */
const MAX_IN_FLIGHT = 4;
let inFlight = 0;
const waiting: Array<() => void> = [];

function next(): void {
  while (inFlight < MAX_IN_FLIGHT && waiting.length) waiting.shift()!();
}

function loadOverview(id: number, done: (text: string) => void, live: () => boolean): void {
  waiting.push(() => {
    if (!live()) return;
    inFlight++;
    api
      .item(id)
      .then((d) => live() && done(d.overview ?? ""), () => undefined)
      .then(() => {
        inFlight--;
        next();
      });
  });
  next();
}

/** The library title page's vertical episode list. */
export function EpisodeList(props: { episodes: ItemSummary[]; nextId?: number }): JSX.Element {
  return (
    <div class="eplist" data-nav-group role="list" aria-label="Episodes">
      <For each={props.episodes}>
        {(ep, i) => <EpisodeRow ep={ep} index={ep.episode_no ?? i() + 1} next={ep.id === props.nextId} />}
      </For>
    </div>
  );
}

function EpisodeRow(props: { ep: ItemSummary; index: number; next: boolean }): JSX.Element {
  let el!: HTMLAnchorElement;
  const [overview, setOverview] = createSignal("");
  const p = () => {
    const v = props.ep.progress;
    return v && v > 0.005 && v < 0.995 && !props.ep.played ? v : 0;
  };

  onMount(() => {
    let alive = true;
    onCleanup(() => (alive = false));
    const start = () => loadOverview(props.ep.id, setOverview, () => alive);
    if (typeof IntersectionObserver === "undefined") return start();
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting || e.intersectionRatio > 0)) return;
        io.disconnect();
        start();
      },
      { rootMargin: "300px 0px" },
    );
    io.observe(el);
    onCleanup(() => io.disconnect());
  });

  return (
    <a
      ref={el}
      class="eprow"
      classList={{ "is-watched": props.ep.played, "is-next": props.next }}
      href={playHref(props.ep)}
      data-fid={props.ep.id}
      data-nav-anchor
      data-nav-default={props.next ? "" : undefined}
      role="listitem"
      aria-label={`Episode ${props.index}: ${props.ep.title}${props.ep.played ? ", watched" : ""}`}
    >
      <span class="eprow__index" aria-hidden="true">{props.index}</span>
      <div class="eprow__still">
        <Img src={props.ep.backdrop_url} width={342} alt="" fallback={<span class="eprow__nostill" />} />
        <span class="eprow__play" aria-hidden="true"><Icon name="play" /></span>
        <Show when={props.ep.played}><span class="card__played"><Icon name="check" /></span></Show>
        <Show when={p()}><div class="card__progress"><i style={{ width: `${p() * 100}%` }} /></div></Show>
      </div>
      <div class="eprow__body">
        <p class="eprow__line">
          <span class="eprow__title">{props.ep.title}</span>
          <Show when={props.ep.runtime_s}><span class="eprow__runtime">{formatRuntime(props.ep.runtime_s)}</span></Show>
        </p>
        <Show when={overview()}><p class="eprow__overview">{overview()}</p></Show>
      </div>
    </a>
  );
}

import { createMemo, createResource, For, Show, type JSX } from "solid-js";
import {
  Backdrop, formatRuntime, formatRemaining, Icon, PosterCard, Row,
  type ItemDetail, type ItemSummary, type Library,
} from "@reel/ui";
import { api } from "../api";
import { usePageFocus } from "../lib/focus";
import { itemActivity } from "../lib/downloads";
import { ErrorState, SkeletonRow, FirstRun } from "../components/States";
import { itemHref, libraryHref, playHref } from "../lib/links";
import { laneHref } from "../lib/browse";
import { continueSubtitle, continueTile } from "../lib/continue";
import { AMBIENT_W, useAmbientArt } from "../lib/ambient";
import { cardPreview } from "../lib/preview";
import type { BrowseLane } from "../api";

export default function Home(): JSX.Element {
  const [data, { refetch }] = createResource(() => Promise.all([api.home(), api.libraries()]));
  const home = () => data()?.[0];
  const libs = () => data()?.[1] ?? [];

  const featuredSummary = createMemo<ItemSummary | undefined>(() => {
    const h = home();
    if (!h) return undefined;
    return h.recently_added.find((i) => i.backdrop_url && i.kind !== "episode") ?? h.recently_added[0];
  });
  const [featured] = createResource(() => featuredSummary()?.id, (id) => api.item(id));

  // Picker lanes from the main film library — a light taste of Browse, loaded after Home.
  const filmLib = () => libs().find((l) => l.kind === "movies" && l.item_count > 0);
  const [picks] = createResource(
    () => (home() ? filmLib()?.id : undefined),
    (id) => api.browseLanes(id).then(pickHomeLanes, () => [] as BrowseLane[]),
  );

  const ambient = useAmbientArt(() => featuredSummary()?.backdrop_url);

  let el!: HTMLDivElement;
  usePageFocus(() => !!home(), () => el);

  const hasItems = () => {
    const h = home();
    return !!h && (h.continue_watching.length > 0 || h.recently_added.length > 0 || h.rows.some((r) => r.items.length > 0));
  };
  const libFor = (id: number): Library | undefined => libs().find((l) => l.id === id);
  return (
    <div class="home" ref={el}>
      <div class="home__phonebar">
        <span class="logo">
          <span class="logo__mark" aria-hidden="true"><i /></span>
          <span class="logo__word">reel</span>
        </span>
      </div>
      <Show when={!data.error} fallback={<ErrorState error={data.error} onRetry={refetch} />}>
        <Show when={!data() || libs().length || hasItems()} fallback={<FirstRun />}>
        <Show when={home()} fallback={<HomeSkeleton />}>
          {(h) => {
            return (
              <>
                <Hero item={featured()} summary={featuredSummary()} />

                <div class="home__rows">
                  <Show when={featuredSummary()?.backdrop_url}>
                    <Backdrop src={ambient()} width={AMBIENT_W} class="home__ambient" />
                    <div class="home__ambient-fade" aria-hidden="true" />
                  </Show>
                  <Show when={h().continue_watching.length}>
                    <Row title="Continue watching" variant="landscape">
                      <For each={h().continue_watching}>
                        {(it) => (
                          <PosterCard
                            item={continueTile(it)}
                            variant="landscape"
                            href={playHref(it)}
                            subtitle={continueSubtitle(it)}
                            preview={cardPreview(playHref(it))}
                          />
                        )}
                      </For>
                    </Row>
                  </Show>
                  <For each={picks() ?? []}>
                    {(lane) => {
                      const more = laneHref(lane, libs());
                      const ranked = lane.kind === "top_unwatched";
                      return (
                        <Row
                          title={lane.title}
                          subtitle={lane.kind === "genre" ? "Pick of the genre" : lane.subtitle}
                          variant={ranked ? "ranked" : "poster"}
                          class="row--lane"
                          action={more ? { label: "Explore all", href: more } : undefined}
                        >
                          <For each={ranked ? lane.items.slice(0, 10) : lane.items}>
                            {(it, n) => (
                              <PosterCard item={it} href={itemHref(it)} rank={ranked ? n() + 1 : undefined} activity={itemActivity(it.id)} preview={cardPreview(itemHref(it))} />
                            )}
                          </For>
                        </Row>
                      );
                    }}
                  </For>
                  <Show when={h().recently_added.length}>
                    <Row title="Recently added">
                      <For each={h().recently_added}>
                        {(it) => <PosterCard item={it} href={itemHref(it)} activity={itemActivity(it.id)} preview={cardPreview(itemHref(it))} />}
                      </For>
                    </Row>
                  </Show>
                  <For each={h().rows}>
                    {(row) => {
                      const lib = libFor(row.library_id);
                      return (
                        <Row
                          title={row.title}
                          action={lib ? { label: "Explore all", href: libraryHref(lib) } : undefined}
                        >
                          <For each={row.items}>
                            {(it) => <PosterCard item={it} href={itemHref(it)} activity={itemActivity(it.id)} preview={cardPreview(itemHref(it))} />}
                          </For>
                        </Row>
                      );
                    }}
                  </For>
                </div>
              </>
            );
          }}
        </Show>
        </Show>
      </Show>
    </div>
  );
}

/** Home keeps it light: Because you watched, Top unwatched, and one genre. */
function pickHomeLanes(all: BrowseLane[]): BrowseLane[] {
  const ok = all.filter((l) => l.items.length >= 4);
  const out: BrowseLane[] = [];
  const take = (kind: string) => {
    const l = ok.find((x) => x.kind === kind && out.indexOf(x) < 0);
    if (l) out.push({ ...l, items: l.items.slice(0, 16) });
  };
  take("because_you_watched");
  take("top_unwatched");
  take("genre");
  return out;
}

function Hero(props: { item?: ItemDetail; summary?: ItemSummary }): JSX.Element {
  const it = () => props.item ?? props.summary;
  const d = () => props.item;
  return (
    <section class="hero" data-nav-anchor style={{ "--nav-anchor-top": "0px" }}>
      <Backdrop src={props.summary?.backdrop_url} class="hero__art" />
      <div class="hero__scrim" aria-hidden="true" />
      <Show when={it()}>
        {(i) => (
          <>
            <div class="hero__body">
              <p class="eyebrow eyebrow--bar">Just added</p>
              <h1 class="hero__title">{i().title}</h1>
              <p class="meta">
                <Show when={i().year}><span>{i().year}</span></Show>
                <Show when={d()?.content_rating}><span class="badge">{d()!.content_rating}</span></Show>
                <Show when={d()?.runtime_s}><span>{formatRuntime(d()!.runtime_s)}</span></Show>
                <Show when={i().kind === "show" && d()?.children_count}>
                  <span>{d()!.children_count} {d()!.children_count === 1 ? "season" : "seasons"}</span>
                </Show>
                <Show when={d()?.genres.length}><span>{d()!.genres.slice(0, 3).join(" / ")}</span></Show>
                <Show when={d()?.rating}>
                  <span class="meta__rating"><Icon name="star" filled /> {d()!.rating!.toFixed(1)}</span>
                </Show>
              </p>
              <p class="hero__overview">{d()?.overview ?? "\u00a0"}</p>
              <div class="actions">
                <Show
                  when={i().kind === "movie"}
                  fallback={
                    <a class="btn btn--primary btn--lg" href={itemHref(i())} data-autofocus>
                      <Icon name="play" /> <span>Play</span>
                    </a>
                  }
                >
                  <a class="btn btn--primary btn--lg" href={playHref(i())} data-autofocus>
                    <Icon name="play" />
                    <span>{i().progress ? "Resume" : "Play"}</span>
                    <Show when={i().progress && d()?.runtime_s}>
                      <small class="btn__aside">{formatRemaining(d()!.runtime_s!, i().progress!)}</small>
                    </Show>
                  </a>
                </Show>
                <a class="btn btn--lg" href={itemHref(i())}>
                  <Icon name="info" /> <span>More info</span>
                </a>
              </div>
            </div>
            <Show when={d()?.content_rating}>
              <div class="hero__flag">{d()!.content_rating}</div>
            </Show>
          </>
        )}
      </Show>
    </section>
  );
}

function HomeSkeleton(): JSX.Element {
  return (
    <div class="home-skeleton" aria-busy="true" aria-label="Loading">
      <section class="hero">
        <div class="hero__body">
          <span class="skel skel--text" style={{ width: "8rem" }} />
          <span class="skel skel--title" />
          <span class="skel skel--text" style={{ width: "20rem" }} />
          <span class="skel skel--text" style={{ width: "28rem" }} />
        </div>
      </section>
      <div class="home__rows">
        <SkeletonRow variant="landscape" count={5} />
        <SkeletonRow count={8} />
      </div>
    </div>
  );
}


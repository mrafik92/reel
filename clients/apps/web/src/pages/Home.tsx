import { createMemo, createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import {
  Backdrop, formatRuntime, formatRemaining, Icon, PosterCard, Row, episodeCode, isTvMode,
  type ItemDetail, type ItemSummary, type Library,
} from "@reel/ui";
import { api } from "../api";
import { usePageFocus } from "../lib/focus";
import { ErrorState, SkeletonRow, FirstRun } from "../components/States";
import { itemHref, libraryHref, playHref } from "../lib/links";

const pad = (n: number) => (n < 10 ? "0" + n : String(n));

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

  // Ambient backdrop follows focus.
  const [focused, setFocused] = createSignal<ItemSummary>();
  const [detail, setDetail] = createSignal<ItemDetail>();
  let dwell: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => dwell && clearTimeout(dwell));
  const onCardFocus = (it: ItemSummary) => {
    setFocused(it);
    if (dwell) clearTimeout(dwell);
    // Fetch detail only once the viewer settles on a title.
    dwell = setTimeout(() => {
      api.item(it.id).then((d) => focused()?.id === it.id && setDetail(d), () => undefined);
    }, 380);
  };
  const onHeroFocus = () => {
    setFocused(undefined);
    setDetail(undefined);
  };
  const backdrop = () => (focused() ?? featuredSummary())?.backdrop_url;
  const captionDetail = () => (detail() && detail()!.id === focused()?.id ? detail() : undefined);

  let el!: HTMLDivElement;
  usePageFocus(() => !!home(), () => el);

  // Scroll state: dims the backdrop on desktop; on TV, rows above the focused
  // one fade out so the backdrop + caption own the top of the screen.
  const [scrolled, setScrolled] = createSignal(false);
  onMount(() => {
    const onScroll = () => setScrolled(window.pageYOffset > window.innerHeight * 0.35);
    window.addEventListener("scroll", onScroll, { passive: true });
    onCleanup(() => window.removeEventListener("scroll", onScroll));
  });
  const onFocusIn = (e: FocusEvent) => {
    if (!isTvMode()) return;
    const row = (e.target as HTMLElement).closest(".home__rows > .row");
    const rows = el.querySelectorAll(".home__rows > .row");
    let past = !!row;
    for (let i = 0; i < rows.length; i++) {
      const r = rows[i]!;
      if (r === row) past = false;
      r.classList.toggle("is-past", past);
    }
  };

  const hasItems = () => {
    const h = home();
    return !!h && (h.continue_watching.length > 0 || h.recently_added.length > 0 || h.rows.some((r) => r.items.length > 0));
  };
  const libFor = (id: number): Library | undefined => libs().find((l) => l.id === id);
  // Editorial row numbering: 01, 02, …
  const idx = createMemo(() => {
    const h = home();
    const cw = h && h.continue_watching.length ? 1 : 0;
    const ra = h && h.recently_added.length ? 1 : 0;
    return { cw: pad(1), ra: pad(cw + 1), rows: (i: number) => pad(cw + ra + i + 1) };
  });

  return (
    <div class="home" ref={el} classList={{ "is-scrolled": scrolled(), "has-focus-row": !!focused() }} onFocusIn={onFocusIn}>
      <Backdrop src={backdrop()} class="page-backdrop" />
      <div class="page-scrim page-scrim--home" aria-hidden="true" />

      <Show when={!data.error} fallback={<ErrorState error={data.error} onRetry={refetch} />}>
        <Show when={!data() || libs().length || hasItems()} fallback={<FirstRun />}>
        <Show when={home()} fallback={<HomeSkeleton />}>
          {(h) => {
            return (
              <>
                <Hero item={featured()} summary={featuredSummary()} onFocus={onHeroFocus} />

                <div class="focus-caption" classList={{ "is-on": !!focused() }} aria-hidden="true">
                  <Show when={focused()}>
                    {(f) => (
                      <>
                        <p class="eyebrow">
                          {f().kind === "episode" ? episodeCode(f()) : f().kind === "show" ? "Series" : "Film"}
                        </p>
                        <h2 class="focus-caption__title">{f().title}</h2>
                        <p class="meta">
                          <Show when={f().year}><span>{f().year}</span></Show>
                          <Show when={captionDetail()?.content_rating}><span class="badge">{captionDetail()!.content_rating}</span></Show>
                          <Show when={captionDetail()?.runtime_s}><span>{formatRuntime(captionDetail()!.runtime_s)}</span></Show>
                          <Show when={captionDetail()?.genres.length}><span>{captionDetail()!.genres.slice(0, 2).join(" / ")}</span></Show>
                        </p>
                        <p class="focus-caption__overview">{captionDetail()?.overview ?? ""}</p>
                      </>
                    )}
                  </Show>
                </div>

                <div class="home__rows">
                  <Show when={h().continue_watching.length}>
                    <Row title="Continue watching" index={idx().cw} variant="landscape">
                      <For each={h().continue_watching}>
                        {(it) => (
                          <PosterCard
                            item={it}
                            variant="landscape"
                            href={playHref(it)}
                            subtitle={
                              (it.kind === "episode" ? episodeCode(it) + " · " : "") +
                              (it.progress ? `${Math.round(it.progress * 100)}% watched` : "")
                            }
                            onFocus={onCardFocus}
                          />
                        )}
                      </For>
                    </Row>
                  </Show>
                  <Show when={h().recently_added.length}>
                    <Row title="Recently added" index={idx().ra}>
                      <For each={h().recently_added}>
                        {(it) => <PosterCard item={it} href={itemHref(it)} onFocus={onCardFocus} />}
                      </For>
                    </Row>
                  </Show>
                  <For each={h().rows}>
                    {(row, i) => {
                      const lib = libFor(row.library_id);
                      return (
                        <Row
                          title={row.title}
                          index={idx().rows(i())}
                          action={lib ? { label: "See all", href: libraryHref(lib) } : undefined}
                        >
                          <For each={row.items}>
                            {(it) => <PosterCard item={it} href={itemHref(it)} onFocus={onCardFocus} />}
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

function Hero(props: { item?: ItemDetail; summary?: ItemSummary; onFocus: () => void }): JSX.Element {
  const it = () => props.item ?? props.summary;
  const d = () => props.item;
  return (
    <section class="hero" data-nav-anchor style={{ "--nav-anchor-top": "0px" }} onFocusIn={props.onFocus}>
      <Show when={it()}>
        {(i) => (
          <div class="hero__body">
            <p class="eyebrow eyebrow--accent">Just added</p>
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
                  <a class="btn btn--primary" href={itemHref(i())} data-autofocus>
                    <Icon name="play" /> <span>Watch now</span>
                  </a>
                }
              >
                <a class="btn btn--primary" href={playHref(i())} data-autofocus>
                  <Icon name="play" />
                  <span>{i().progress ? "Resume" : "Play"}</span>
                  <Show when={i().progress && d()?.runtime_s}>
                    <small class="btn__aside">{formatRemaining(d()!.runtime_s!, i().progress!)}</small>
                  </Show>
                </a>
              </Show>
              <a class="btn" href={itemHref(i())}>
                <Icon name="info" /> <span>More info</span>
              </a>
            </div>
          </div>
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


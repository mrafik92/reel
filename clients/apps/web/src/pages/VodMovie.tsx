import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { useParams } from "@solidjs/router";
import { Icon, Row, formatRemaining, formatRuntime } from "@reel/ui";
import { vod, type VodMovie as Movie } from "../api/vod";
import { usePageFocus } from "../lib/focus";
import { itemHref, playHref } from "../lib/links";
import { ErrorState } from "../components/States";
import { RatingsStrip, ReviewsSection, useReviews } from "../components/Reviews";
import { CastTile } from "../components/Cast";
import { vodPlayHref } from "../components/vod/links";
import { TrailerModal, VodEyebrow, VodFineprint, VodSkeleton } from "../components/vod/Detail";
import { TitleSheet } from "../components/TitleSheet";

/** An IPTV film: modeled on the library detail page, played through the provider. */
export default function VodMovie(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [m, { refetch }] = createResource(() => params.id, (id) => vod.movie(id));
  let el!: HTMLDivElement;
  usePageFocus(() => !!m() || !!m.error, () => el);

  return (
    <div class="page detail vdetail" ref={el}>
      <Show when={!m.error} fallback={<ErrorState error={m.error} title="Couldn’t open this IPTV title" onRetry={refetch} />}>
        <Show when={m()} fallback={<VodSkeleton />}>
          {(x) => <MovieBody m={x()} />}
        </Show>
      </Show>
    </div>
  );
}

function MovieBody(props: { m: Movie }): JSX.Element {
  const m = () => props.m;
  const progress = () => (!m().played && m().progress && m().progress! > 0.005 && m().progress! < 0.97 ? m().progress! : 0);
  const resumeAt = () => (progress() && m().runtime_s ? progress() * m().runtime_s! : 0);
  const local = () => m().library_item_id ?? null;
  const reviews = useReviews(() => (m().tmdb_id ? { media: "movie", tmdbId: m().tmdb_id! } : undefined));
  const [trailer, setTrailer] = createSignal(false);

  const head = (
    <>
        <VodEyebrow kind="movie" categoryId={m().category_id} />
        <h1 class="detail__title" dir="auto">{m().title}</h1>
        <div class="actions tsheet__actions" data-nav-group>
          <Show
            when={local()}
            fallback={
              <>
                <a class="btn btn--primary btn--lg" href={vodPlayHref("movie", m().id, { t: resumeAt() })} data-autofocus>
                  <Icon name="play" />
                  <span>{progress() ? "Resume" : "Play"}</span>
                  <Show when={progress() && m().runtime_s}>
                    <small class="btn__aside">{formatRemaining(m().runtime_s!, progress())}</small>
                  </Show>
                </a>
                <Show when={progress()}>
                  <a class="btn" href={vodPlayHref("movie", m().id, { fromStart: true })}>
                    <Icon name="restart" /> <span>From the beginning</span>
                  </a>
                </Show>
              </>
            }
          >
            <a class="btn btn--primary btn--lg" href={playHref({ id: local()! })} data-autofocus>
              <Icon name="play" /> <span>Play</span> <small class="btn__aside">from your library</small>
            </a>
            <a class="btn" href={vodPlayHref("movie", m().id, { t: resumeAt() })}>
              <Icon name="live" /> <span>{progress() ? "Resume on IPTV" : "Play from IPTV"}</span>
            </a>
            <a class="btn btn--ghost" href={itemHref({ id: local()!, kind: "movie" })}>
              <Icon name="info" /> <span>Library details</span>
            </a>
          </Show>
          <Show when={m().trailer_youtube_id}>
            <button type="button" class="btn" onClick={() => setTrailer(true)}>
              <Icon name="film" /> <span>Trailer</span>
            </button>
          </Show>
        </div>
    </>
  );

  return (
    <TitleSheet backdrop={m().backdrop_url} head={head}>
      <section class="tinfo" data-nav-anchor>
        <div class="tinfo__main">
        <p class="meta">
          <Show when={m().year}><span>{m().year}</span></Show>
          <Show when={m().runtime_s}><span>{formatRuntime(m().runtime_s)}</span></Show>
          <Show when={(m().genres ?? []).length}><span>{m().genres!.slice(0, 3).join(" / ")}</span></Show>
          <Show when={m().rating}>
            <span class="meta__rating"><Icon name="star" filled /> {m().rating!.toFixed(1)}</span>
          </Show>
          <Show when={m().container}><span class="badge">{m().container!.toUpperCase()}</span></Show>
          <Show when={m().played}><span class="meta__watched"><Icon name="check" /> Watched</span></Show>
        </p>
        <RatingsStrip data={reviews()} />
        <Show when={local()}>
          <p class="vlib"><Icon name="check" /> <span><strong>Also in your library.</strong> Plays from your own server in full quality.</span></p>
        </Show>
        <Show when={m().overview}>
          <p class="detail__overview" dir="auto">{m().overview}</p>
        </Show>
        <Show when={m().director}>
          <p class="vcredit">Directed by <strong dir="auto">{m().director}</strong></p>
        </Show>

        <VodFineprint local={!!local()} />
        </div>
      </section>

      <Show when={m().cast.length}>
        <Row title="Cast" variant="cast">
          <For each={m().cast.slice(0, 20)}>{(c) => <CastTile c={c} />}</For>
        </Row>
      </Show>

      <ReviewsSection data={reviews()} />

      <Show when={trailer() && m().trailer_youtube_id}>
        <TrailerModal youtubeId={m().trailer_youtube_id!} title={m().title} onClose={() => setTrailer(false)} />
      </Show>
    </TitleSheet>
  );
}

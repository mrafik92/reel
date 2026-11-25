import { createMemo, createResource, createSignal, For, Show, type JSX } from "solid-js";
import { useParams } from "@solidjs/router";
import { Icon, Img, Row, formatRuntime } from "@reel/ui";
import { vod, type VodEpisode, type VodSeason, type VodSeries as Series } from "../api/vod";
import { usePageFocus } from "../lib/focus";
import { itemHref } from "../lib/links";
import { ErrorState } from "../components/States";
import { RatingsStrip, ReviewsSection, useReviews } from "../components/Reviews";
import { CastTile } from "../components/Cast";
import { vodPlayHref } from "../components/vod/links";
import { VodEyebrow, VodFineprint, VodSkeleton } from "../components/vod/Detail";
import { TrailerModal } from "../components/Trailer";
import { SeasonPicker, TitleSheet } from "../components/TitleSheet";

/** An IPTV series: hero, season tabs, and the episode list with progress. */
export default function VodSeries(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [s, { refetch }] = createResource(() => params.id, (id) => vod.series(id));
  let el!: HTMLDivElement;
  usePageFocus(() => !!s() || !!s.error, () => el);

  return (
    <div class="page detail vdetail" ref={el}>
      <Show when={!s.error} fallback={<ErrorState error={s.error} title="Couldn’t open this IPTV series" onRetry={refetch} />}>
        <Show when={s()} fallback={<VodSkeleton />}>
          {(x) => <SeriesBody s={x()} />}
        </Show>
      </Show>
    </div>
  );
}

const live = (e: VodEpisode) => !e.played && e.progress && e.progress > 0.005 && e.progress < 0.97;

/** Where to pick up: an episode in progress, else the one after the last watched, else the start. */
function nextUp(seasons: VodSeason[]): { season: VodSeason; ep: VodEpisode } | undefined {
  const flat: { season: VodSeason; ep: VodEpisode }[] = [];
  seasons.forEach((season) => season.episodes.forEach((ep) => flat.push({ season, ep })));
  if (!flat.length) return undefined;
  const going = flat.find((x) => live(x.ep));
  if (going) return going;
  let last = -1;
  flat.forEach((x, i) => x.ep.played && (last = i));
  return flat[last + 1] ?? flat[0];
}

function SeriesBody(props: { s: Series }): JSX.Element {
  const s = () => props.s;
  const seasons = () => s().seasons.slice().sort((a, b) => a.season_no - b.season_no);
  const up = createMemo(() => nextUp(seasons()));
  const [seasonNo, setSeasonNo] = createSignal<number>(up()?.season.season_no ?? seasons()[0]?.season_no ?? 1);
  const season = () => seasons().find((x) => x.season_no === seasonNo()) ?? seasons()[0];
  const local = () => s().library_item_id ?? null;
  const reviews = useReviews(() => (s().tmdb_id ? { media: "tv", tmdbId: s().tmdb_id! } : undefined));
  const [trailer, setTrailer] = createSignal(false);
  const nEpisodes = () => seasons().reduce((a, x) => a + x.episodes.length, 0);
  const code = (sn: number, en: number) => `S${sn} E${en}`;
  const started = () => seasons().some((x) => x.episodes.some((e) => e.played || live(e)));
  const playEp = (_sn: VodSeason, ep: VodEpisode, fromStart = false) =>
    vodPlayHref("episode", ep.id, { series: s().id, fromStart, t: !fromStart && live(ep) && ep.runtime_s ? ep.progress! * ep.runtime_s : 0 });

  const head = (
    <>
        <VodEyebrow kind="series" categoryId={s().category_id} />
        <h1 class="detail__title" dir="auto">{s().title}</h1>
        <div class="actions tsheet__actions" data-nav-group>
          <Show when={local()}>
            <a class="btn btn--primary btn--lg" href={itemHref({ id: local()!, kind: "show" })} data-autofocus>
              <Icon name="play" /> <span>Watch from your library</span>
            </a>
          </Show>
          <Show when={up()} fallback={<p class="note"><Icon name="alert" /> Your provider lists no episodes for this series yet.</p>}>
            {(u) => (
              <a class={local() ? "btn" : "btn btn--primary btn--lg"} href={playEp(u().season, u().ep)} data-autofocus={local() ? undefined : ""}>
                <Icon name={local() ? "live" : "play"} />
                <span>
                  {live(u().ep) ? "Resume" : started() ? "Continue" : "Play"}{" "}
                  {code(u().season.season_no, u().ep.episode_no)}
                </span>
                <Show when={local()}><small class="btn__aside">on IPTV</small></Show>
              </a>
            )}
          </Show>
          <Show when={s().trailer_youtube_id}>
            <button type="button" class="btn" onClick={() => setTrailer(true)}>
              <Icon name="film" /> <span>Trailer</span>
            </button>
          </Show>
        </div>
    </>
  );

  return (
    <TitleSheet backdrop={s().backdrop_url} head={head}>
      <section class="tinfo" data-nav-anchor>
        <div class="tinfo__main">
        <p class="meta">
          <Show when={s().year}><span>{s().year}</span></Show>
          <Show when={seasons().length}>
            <span>{seasons().length} {seasons().length === 1 ? "season" : "seasons"} · {nEpisodes()} episodes</span>
          </Show>
          <Show when={(s().genres ?? []).length}><span>{s().genres!.slice(0, 3).join(" / ")}</span></Show>
          <Show when={s().rating}>
            <span class="meta__rating"><Icon name="star" filled /> {s().rating!.toFixed(1)}</span>
          </Show>
        </p>
        <RatingsStrip data={reviews()} />
        <Show when={local()}>
          <p class="vlib"><Icon name="check" /> <span><strong>Also in your library.</strong> Open it there to play from your own server.</span></p>
        </Show>
        <Show when={s().overview}>
          <p class="detail__overview" dir="auto">{s().overview}</p>
        </Show>

        <VodFineprint local={!!local()} />
        </div>
      </section>

      <Show when={seasons().length}>
        <section class="tepisodes" data-nav-anchor>
          <header class="tepisodes__head">
            <h2 class="section-title">Episodes</h2>
            <SeasonPicker
              seasons={seasons().map((sn) => ({
                value: sn.season_no,
                label: /^season \d+$/i.test(sn.name ?? "") || !sn.name ? `Season ${sn.season_no}` : sn.name,
              }))}
              selected={seasonNo()}
              onSelect={(v) => setSeasonNo(v)}
            />
          </header>
          <Show when={season()} keyed>
            {(sn) => (
              <div class="eplist" data-nav-group role="list" aria-label={`Season ${sn.season_no} episodes`}>
                <For each={sn.episodes}>
                  {(ep) => <EpisodeRow season={sn} ep={ep} href={playEp(sn, ep)} next={up()?.ep.id === ep.id} />}
                </For>
              </div>
            )}
          </Show>
        </section>
      </Show>

      <Show when={s().cast.length}>
        <Row title="Cast" variant="cast">
          <For each={s().cast.slice(0, 20)}>{(c) => <CastTile c={c} />}</For>
        </Row>
      </Show>

      <ReviewsSection data={reviews()} />

      <Show when={trailer() && s().trailer_youtube_id}>
        <TrailerModal youtubeId={s().trailer_youtube_id!} title={s().title} onClose={() => setTrailer(false)} />
      </Show>
    </TitleSheet>
  );
}

function EpisodeRow(props: { season: VodSeason; ep: VodEpisode; href: string; next: boolean }): JSX.Element {
  const e = () => props.ep;
  const p = () => (live(e()) ? e().progress! : 0);
  // Providers often name episodes "Episode 4" / "الحلقة 4": show the number once, not twice.
  const generic = () => /^(episode|ep\.?|الحلقة)\s*\d+$/i.test(e().title.trim());
  const left = () => (p() && e().runtime_s ? Math.round(((1 - p()) * e().runtime_s!) / 60) : 0);
  return (
    <a
      class="eprow"
      classList={{ "is-watched": !!e().played, "is-next": props.next }}
      href={props.href}
      data-fid={`ve:${e().id}`}
      data-nav-anchor
      data-nav-default={props.next ? "" : undefined}
      role="listitem"
      aria-label={`Episode ${e().episode_no}${generic() ? "" : ": " + e().title}${e().played ? ", watched" : p() ? `, ${left()} minutes left` : ""}`}
    >
      <span class="eprow__index" aria-hidden="true">{e().episode_no}</span>
      <div class="eprow__still">
        <Img src={e().still_url} width={342} alt="" fallback={<span class="eprow__nostill" />} />
        <span class="eprow__play" aria-hidden="true"><Icon name="play" /></span>
        <Show when={e().played}><span class="card__played"><Icon name="check" /></span></Show>
        <Show when={p()}><div class="card__progress"><i style={{ width: `${p() * 100}%` }} /></div></Show>
      </div>
      <div class="eprow__body">
        <p class="eprow__line">
          <span class="eprow__title" dir="auto">{generic() ? `Episode ${e().episode_no}` : e().title}</span>
          <Show when={p()} fallback={<Show when={e().runtime_s}><span class="eprow__runtime">{formatRuntime(e().runtime_s)}</span></Show>}>
            <span class="eprow__runtime">{left()} min left</span>
          </Show>
        </p>
        <Show when={e().overview}><p class="eprow__overview" dir="auto">{e().overview}</p></Show>
      </div>
    </a>
  );
}

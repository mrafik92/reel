import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon, Img, isTvMode } from "@reel/ui";
import { api, type MediaType, type Rating, type RatingSource, type Review, type Reviews } from "../api";

/** Lazily fetched ratings/reviews; failures (older server, no data) resolve to empty. */
export function useReviews(key: () => { media: MediaType; tmdbId: number } | undefined) {
  const [r] = createResource(key, (k) => api.reviews(k.media, k.tmdbId).catch((): Reviews => ({ ratings: [], reviews: [], links: [] })));
  return r;
}

const fmtVotes = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));

function Badge(props: { r: Rating }): JSX.Element {
  const score = () => props.r.score_100 ?? null;
  const tone = () => (score() == null ? "" : score()! >= 60 ? "good" : score()! >= 40 ? "mid" : "bad");
  const v = () => props.r.value.replace(/\/10$/, "").replace(/\/100$/, "");
  switch (props.r.source) {
    case "imdb":
      return (
        <span class="rating rating--imdb" title={`IMDb ${props.r.value}${props.r.votes ? ` · ${props.r.votes.toLocaleString()} votes` : ""}`}>
          <b class="rating__mark">IMDb</b>
          <span class="rating__value">{v()}</span>
          <Show when={props.r.votes}><small>{fmtVotes(props.r.votes!)}</small></Show>
        </span>
      );
    case "rotten_tomatoes":
      return (
        <span class={`rating rating--rt rating--${tone()}`} title={`Rotten Tomatoes ${props.r.value}`}>
          {/* Fresh = tomato, rotten = green splat (drawn in CSS, no logo assets). */}
          <i class="rating__tomato" aria-hidden="true" />
          <span class="rating__value">{props.r.value}</span>
          <small>Tomatometer</small>
        </span>
      );
    case "metacritic":
      return (
        <span class={`rating rating--mc rating--${tone()}`} title={`Metacritic ${props.r.value}`}>
          <b class="rating__mark">{v()}</b>
          <small>Metascore</small>
        </span>
      );
    case "tmdb":
      return (
        <span class="rating rating--tmdb" title={`TMDB user score ${props.r.value}`}>
          <b class="rating__mark">TMDB</b>
          <span class="rating__value">{props.r.value}</span>
        </span>
      );
    default:
      // A source this client doesn't know yet: show it plainly.
      return (
        <span class="rating" title={`${props.r.label} ${props.r.value}`}>
          <b class="rating__mark rating__mark--plain">{props.r.label}</b>
          <span class="rating__value">{props.r.value}</span>
        </span>
      );
  }
}

/** IMDb ★ · RT 🍅 · Metacritic · TMDB, in that order. */
export function RatingsStrip(props: { data?: Reviews }): JSX.Element {
  const order: RatingSource[] = ["imdb", "rotten_tomatoes", "metacritic", "tmdb"];
  const rank = (s: string) => {
    const i = order.indexOf(s as RatingSource);
    return i < 0 ? order.length : i;
  };
  const sorted = () => (props.data?.ratings ?? []).slice().sort((a, b) => rank(a.source) - rank(b.source));
  return (
    <Show when={sorted().length}>
      <div class="ratings" aria-label="Ratings">
        <For each={sorted()}>{(r) => <Badge r={r} />}</For>
      </div>
    </Show>
  );
}

const LINK_LABEL: Record<string, string> = {
  imdb: "IMDb", rotten_tomatoes: "Rotten Tomatoes", letterboxd: "Letterboxd", tmdb: "TMDB", trakt: "Trakt",
};
const SOURCE_LABEL: Record<string, string> = { tmdb: "TMDB", trakt: "Trakt" };

/** Reviews are user-written markdown-ish text: show it plain. */
function clean(s: string): string {
  return s.replace(/\r\n?/g, "\n").replace(/\*\*(.+?)\*\*/g, "$1").replace(/__(.+?)__/g, "$1").replace(/<[^>]+>/g, "").trim();
}

function ReviewCard(props: { r: Review; compact?: boolean }): JSX.Element {
  const [open, setOpen] = createSignal(false);
  const text = () => clean(props.r.content);
  const long = () => text().length > (props.compact ? 220 : 360);
  const date = () => {
    const d = new Date(props.r.created_at);
    return isNaN(d.getTime()) ? "" : d.toLocaleDateString(undefined, { year: "numeric", month: "short" });
  };
  return (
    // The whole card is the focus target (remotes scroll card to card); Enter expands long ones.
    <article
      class="review"
      classList={{ "is-open": open(), "is-compact": !!props.compact, "is-long": long() }}
      tabindex="0"
      data-focusable
      aria-expanded={long() ? open() : undefined}
      onClick={() => long() && setOpen((v) => !v)}
    >
      <header class="review__head">
        <Img
          class="review__avatar"
          src={props.r.avatar_url}
          width={92}
          fallback={<span class="review__initial">{props.r.author.charAt(0).toUpperCase()}</span>}
        />
        <div class="review__who">
          <strong>{props.r.author}</strong>
          <span>{SOURCE_LABEL[props.r.source] ?? props.r.source}{date() ? ` · ${date()}` : ""}</span>
        </div>
        <Show when={props.r.rating != null}>
          <span class="review__rating"><Icon name="star" filled /> {props.r.rating!.toFixed(props.r.rating! % 1 ? 1 : 0)}<small>/10</small></span>
        </Show>
      </header>
      <p class="review__text">{text()}</p>
      <Show when={long()}>
        <span class="review__more">{open() ? "Show less" : "Read full review"}</span>
      </Show>
    </article>
  );
}

/** "Reviews" section + external links. On TV the links are hidden (no browser to open). */
export function ReviewsSection(props: { data?: Reviews; compact?: boolean; limit?: number }): JSX.Element {
  const reviews = () => (props.data?.reviews ?? []).slice(0, props.limit ?? 12);
  const links = () => (isTvMode() ? [] : (props.data?.links ?? []).filter((l) => l.source !== "tmdb"));
  return (
    <Show when={reviews().length || links().length}>
      <section class="reviews" classList={{ "reviews--compact": !!props.compact }} data-nav-anchor>
        <Show when={reviews().length}>
          <h2 class="section-title">
            Reviews <span class="section-title__aside">{props.data!.reviews.length}</span>
          </h2>
          <div class="reviews__list" data-nav-group>
            <For each={reviews()}>{(r) => <ReviewCard r={r} compact={props.compact} />}</For>
          </div>
        </Show>
        <Show when={links().length}>
          <p class="reviews__links">
            <span>Read more on</span>
            <For each={links()}>
              {(l) => (
                <a class={`extlink extlink--${l.source}`} href={l.url} target="_blank" rel="noopener noreferrer">
                  <Show when={l.source === "letterboxd"}>
                    <i class="lbx" aria-hidden="true"><b /><b /><b /></i>
                  </Show>
                  {LINK_LABEL[l.source] ?? l.source}
                  <Icon name="forward" />
                </a>
              )}
            </For>
          </p>
        </Show>
      </section>
    </Show>
  );
}

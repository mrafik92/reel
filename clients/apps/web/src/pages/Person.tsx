import { createMemo, createResource, createSignal, For, Show, type JSX } from "solid-js";
import { useParams } from "@solidjs/router";
import { Icon, Img, isTvMode } from "@reel/ui";
import { api, ApiError, type PersonCredit } from "../api";
import { isAdmin } from "../lib/auth";
import { primeFromCards } from "../lib/downloads";
import { usePageFocus } from "../lib/focus";
import { DiscoverTile, useDiscoverSheet } from "../components/Discover";
import { EmptyState, ErrorState } from "../components/States";

type Tab = "all" | "movie" | "tv";
type Sort = "newest" | "oldest" | "rated" | "popular";

interface Merged {
  credit: PersonCredit;
  roles: string[];
  /** Position in the server's newest-first order (tie-breaker, and the "newest" order itself). */
  i: number;
}

const SORTS: [Sort, string][] = [["newest", "Newest"], ["oldest", "Oldest"], ["rated", "Top rated"], ["popular", "Popular"]];
const SORT_KEY = "reel.person.sort";
/**
 * "Top rated" only trusts a TMDB average backed by this many votes; thinner ones go after,
 * so a 10.0 from 3 votes can't top the list. 50 votes keeps the average within a few tenths
 * of where it settles, and an actor's notable films clear it easily.
 */
const MIN_VOTES = 50;

function loadSort(): Sort {
  try {
    const v = localStorage.getItem(SORT_KEY);
    return SORTS.some(([s]) => s === v) ? (v as Sort) : "newest";
  } catch {
    return "newest";
  }
}

const trusted = (c: PersonCredit) => !!c.vote_average && (c.vote_count ?? 0) >= MIN_VOTES;
/** 0 = trusted rating, 1 = thin rating, 2 = unrated. */
const ratingTier = (c: PersonCredit) => (trusted(c) ? 0 : c.vote_average ? 1 : 2);

function sortCredits(list: Merged[], by: Sort): Merged[] {
  const out = list.slice();
  const cmp: Record<Sort, (a: Merged, b: Merged) => number> = {
    newest: (a, b) => a.i - b.i,
    // Undated titles stay last; same year → earlier release first (server order reversed).
    oldest: (a, b) => (a.credit.year ?? Infinity) - (b.credit.year ?? Infinity) || b.i - a.i,
    rated: (a, b) =>
      ratingTier(a.credit) - ratingTier(b.credit) ||
      (b.credit.vote_average ?? 0) - (a.credit.vote_average ?? 0) ||
      (b.credit.vote_count ?? 0) - (a.credit.vote_count ?? 0) ||
      a.i - b.i,
    // Vote count, not TMDB `popularity`: popularity scores the whole title, so a long-running talk show
    // with one guest spot outranks the films someone is known for. Popularity only breaks ties.
    popular: (a, b) =>
      (b.credit.vote_count ?? -1) - (a.credit.vote_count ?? -1) ||
      (b.credit.popularity ?? -1) - (a.credit.popularity ?? -1) ||
      a.i - b.i,
  };
  return out.sort(cmp[by]);
}

function fmtDate(iso: string): string {
  const d = new Date(iso + "T12:00:00Z");
  return isNaN(d.getTime()) ? iso : d.toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" });
}
function age(from: string, to?: string | null): number | undefined {
  const a = new Date(from), b = to ? new Date(to) : new Date();
  if (isNaN(a.getTime()) || isNaN(b.getTime())) return undefined;
  let y = b.getFullYear() - a.getFullYear();
  if (b.getMonth() < a.getMonth() || (b.getMonth() === a.getMonth() && b.getDate() < a.getDate())) y--;
  return y;
}

export default function Person(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [p, { refetch }] = createResource(() => Number(params.id), (id) => api.person(id));
  const [tab, setTab] = createSignal<Tab>("all");
  const [libOnly, setLibOnly] = createSignal(false);
  const [sort, setSortSig] = createSignal<Sort>(loadSort());
  const setSort = (v: Sort) => {
    setSortSig(v);
    try {
      localStorage.setItem(SORT_KEY, v);
    } catch {
      /* private mode etc.: the choice just isn't remembered */
    }
  };
  const [bioOpen, setBioOpen] = createSignal(false);
  const sheet = useDiscoverSheet();

  /** One tile per title; several jobs/characters are folded into the subtitle. */
  const merged = createMemo<Merged[]>(() => {
    const person = p.error ? undefined : p();
    if (!person) return [];
    primeFromCards(person.credits);
    const m = new Map<string, Merged>();
    for (const c of person.credits) {
      const k = `${c.media_type}:${c.tmdb_id}`;
      const role = c.role === "cast" ? (c.character ? `as ${c.character}` : "Cast") : c.job || "Crew";
      const hit = m.get(k);
      if (hit) {
        if (hit.roles.indexOf(role) < 0) hit.roles.push(role);
      } else m.set(k, { credit: c, roles: [role], i: m.size });
    }
    return Array.from(m.values());
  });
  const filtered = createMemo(() =>
    sortCredits(
      merged().filter((x) =>
        (tab() === "all" || x.credit.media_type === tab()) &&
        (!libOnly() || x.credit.status === "available" || x.credit.status === "partially")),
      sort(),
    ));
  /** Card subtitle: year and part, led by the TMDB rating while sorting by rating. */
  const subtitle = (x: Merged) => {
    const c = x.credit;
    const rating = sort() === "rated" && c.vote_average ? `★ ${c.vote_average.toFixed(1)}${trusted(c) ? "" : " (few votes)"}` : "";
    return [rating, c.year ? String(c.year) : "", x.roles.join(", ")].filter(Boolean).join(" · ");
  };
  const count = (t: Tab) => merged().filter((x) => t === "all" || x.credit.media_type === t).length;
  const inLibrary = () => merged().filter((x) => x.credit.status === "available" || x.credit.status === "partially").length;

  let el!: HTMLDivElement;
  usePageFocus(() => !!p.error || !!p(), () => el);

  return (
    <div class="page person-page" ref={el}>
      <Show when={!p.error} fallback={<PersonError error={p.error} onRetry={refetch} />}>
        <Show when={p()} fallback={<PersonSkeleton />}>
          {(person) => {
            const bio = () => (person().biography ?? "").trim();
            const longBio = () => bio().length > 420;
            return (
              <>
                <header class="person-head">
                  <Img
                    class="person-head__photo"
                    src={person().photo_url}
                    width={342}
                    eager
                    fallback={<span class="person-head__initials">{person().name.split(" ").map((w) => w.charAt(0)).join("").slice(0, 2)}</span>}
                  />
                  <div class="person-head__body">
                    <p class="eyebrow eyebrow--accent">{person().known_for_department ?? "Person"}</p>
                    <h1 class="person-head__name">{person().name}</h1>
                    <p class="meta">
                      <Show when={person().birthday}>
                        <span>
                          Born {fmtDate(person().birthday!)}
                          {person().place_of_birth ? ` in ${person().place_of_birth}` : ""}
                          {!person().deathday && age(person().birthday!) != null ? ` (age ${age(person().birthday!)})` : ""}
                        </span>
                      </Show>
                      <Show when={person().deathday}>
                        <span>Died {fmtDate(person().deathday!)}{person().birthday && age(person().birthday!, person().deathday) != null ? ` (aged ${age(person().birthday!, person().deathday)})` : ""}</span>
                      </Show>
                    </p>
                    <Show when={bio()}>
                      <p class="person-head__bio" classList={{ "is-open": bioOpen() || !longBio() }}>{bio()}</p>
                      <Show when={longBio()}>
                        <button type="button" class="btn btn--quiet btn--sm person-head__more" aria-expanded={bioOpen()} onClick={() => setBioOpen((v) => !v)}>
                          <Icon name={bioOpen() ? "back" : "down"} /> <span>{bioOpen() ? "Show less" : "Read full biography"}</span>
                        </button>
                      </Show>
                    </Show>
                    <Show when={person().imdb_id && !isTvMode()}>
                      <a class="extlink extlink--imdb" href={`https://www.imdb.com/name/${person().imdb_id}/`} target="_blank" rel="noopener noreferrer">
                        IMDb <Icon name="forward" />
                      </a>
                    </Show>
                  </div>
                </header>

                <section class="person-credits" data-nav-anchor>
                  <div class="person-credits__bar">
                    <h2 class="section-title">Filmography <span class="section-title__aside">{merged().length} titles · {inLibrary()} in your library</span></h2>
                    <div class="person-credits__filters" data-nav-group>
                      <div class="segmented" role="group" aria-label="Show">
                        <For each={[["all", "All"], ["movie", "Films"], ["tv", "Series"]] as [Tab, string][]}>
                          {([t, label]) => (
                            <button type="button" class="segmented__btn" aria-pressed={tab() === t} onClick={() => setTab(t)}>
                              {label} <span class="segmented__count">{count(t)}</span>
                            </button>
                          )}
                        </For>
                      </div>
                      <button type="button" class="chip chip--toggle" aria-pressed={libOnly()} onClick={() => setLibOnly((v) => !v)}>
                        <Show when={libOnly()}><Icon name="check" /></Show> In your library
                      </button>
                      <div class="segmented person-credits__sort" role="group" aria-label="Sort by">
                        <For each={SORTS}>
                          {([v, label]) => (
                            <button
                              type="button"
                              class="segmented__btn"
                              aria-pressed={sort() === v}
                              title={v === "rated" ? `TMDB rating; titles with fewer than ${MIN_VOTES} votes come after` : v === "popular" ? "Most TMDB votes first" : undefined}
                              onClick={() => setSort(v)}
                            >
                              {label}
                            </button>
                          )}
                        </For>
                      </div>
                    </div>
                  </div>
                  <Show
                    when={filtered().length}
                    fallback={<EmptyState title={libOnly() ? "None of these are in your library yet" : "No titles here"} body={libOnly() ? "Turn off “In your library” to see everything — and add what you like." : undefined} />}
                  >
                    <div class="results-grid results-grid--discover" data-nav-group="free">
                      <For each={filtered()}>
                        {(x, i) => (
                          <DiscoverTile
                            card={x.credit}
                            onOpen={sheet.open}
                            autofocus={i() === 0}
                            subtitle={subtitle(x)}
                          />
                        )}
                      </For>
                    </div>
                  </Show>
                </section>
              </>
            );
          }}
        </Show>
      </Show>
      {sheet.view()}
    </div>
  );
}

/** 404 = TMDB doesn't know this person; 424 = server has no TMDB key. */
function PersonError(props: { error: unknown; onRetry: () => void }): JSX.Element {
  const status = () => (props.error instanceof ApiError ? props.error.status : 0);
  return (
    <Show when={status() === 404 || status() === 424} fallback={<ErrorState error={props.error} onRetry={props.onRetry} />}>
      <EmptyState
        title={status() === 404 ? "We couldn’t find this person" : "Filmographies need TMDB"}
        body={
          status() === 404
            ? "TMDB has no record of them — the cast list may come from a local .nfo file."
            : isAdmin()
              ? "Add a TMDB API key to the server config (tmdb_api_key, or REEL_TMDB_API_KEY) and restart it."
              : "Your server’s admin needs to connect it to TMDB first."
        }
      >
        <button type="button" class="btn" data-autofocus onClick={() => window.history.back()}>
          <Icon name="back" /> <span>Go back</span>
        </button>
      </EmptyState>
    </Show>
  );
}

function PersonSkeleton(): JSX.Element {
  return (
    <header class="person-head" aria-busy="true" aria-label="Loading">
      <div class="person-head__photo skel" />
      <div class="person-head__body">
        <span class="skel skel--text" style={{ width: "6rem" }} />
        <span class="skel skel--title" />
        <span class="skel skel--text" style={{ width: "24rem" }} />
        <span class="skel skel--text" style={{ width: "36rem" }} />
      </div>
    </header>
  );
}

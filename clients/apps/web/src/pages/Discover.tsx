import { createResource, For, Show, type JSX } from "solid-js";
import { Icon, Row } from "@reel/ui";
import { api, ApiError, type DiscoverCard, type DiscoverList } from "../api";
import { primeFromCards } from "../lib/downloads";
import { usePageFocus } from "../lib/focus";
import { isAdmin } from "../lib/auth";
import { href } from "../lib/mode";
import { DiscoverTile, useDiscoverSheet } from "../components/Discover";
import { ErrorState, SkeletonRow } from "../components/States";

const ROWS: { title: string; list: DiscoverList; media: "movie" | "tv" | "all" }[] = [
  { title: "Trending this week", list: "trending", media: "all" },
  { title: "Popular films", list: "popular", media: "movie" },
  { title: "Popular series", list: "popular", media: "tv" },
  { title: "Coming soon", list: "upcoming", media: "movie" },
];

const pad = (n: number) => (n < 10 ? "0" + n : String(n));

export default function Discover(): JSX.Element {
  const [rows, { refetch }] = createResource(async () => {
    const pages = await Promise.all(ROWS.map((r) => api.discover(r.list, r.media).catch((e: unknown) => e)));
    // TMDB missing on the server → one clear message instead of four broken rows.
    const notConfigured = pages.find((p) => p instanceof ApiError && p.status === 424);
    if (notConfigured) throw notConfigured;
    const firstErr = pages.find((p) => p instanceof Error);
    if (firstErr && pages.every((p) => p instanceof Error)) throw firstErr;
    const out = ROWS.map((r, i) => ({ ...r, cards: pages[i] instanceof Error ? [] : (pages[i] as { results: DiscoverCard[] }).results }));
    out.forEach((r) => primeFromCards(r.cards));
    return out.filter((r) => r.cards.length);
  });
  const sheet = useDiscoverSheet();
  let el!: HTMLDivElement;
  usePageFocus(() => !!rows.error || !!rows(), () => el);

  const tmdbMissing = () => rows.error instanceof ApiError && rows.error.status === 424;

  return (
    <div class="page discover" ref={el}>
      <header class="page-head">
        <p class="eyebrow eyebrow--accent">Discover</p>
        <div class="page-head__row">
          <h1 class="page-title">Find something new</h1>
          <a class="btn" href={href("/search")}>
            <Icon name="search" /> <span>Search everything</span>
          </a>
        </div>
        <p class="page-lede">Add a title and it downloads straight into your library — watch the ring fill, then press Play.</p>
      </header>

      <Show when={!rows.error} fallback={
        <Show when={tmdbMissing()} fallback={<ErrorState error={rows.error} onRetry={refetch} />}>
          <div class="state">
            <h2 class="state__title">Discover isn't connected yet</h2>
            <p class="state__body">
              {isAdmin()
                ? "Add a TMDB API key to the server config (tmdb_api_key, or the REEL_TMDB_API_KEY environment variable) and restart it."
                : "Your server's admin needs to connect it to TMDB first."}
            </p>
          </div>
        </Show>
      }>
        <Show when={rows()} fallback={<><SkeletonRow /><SkeletonRow /></>}>
          <div class="discover__rows">
            <For each={rows()}>
              {(r, i) => (
                <Row title={r.title} index={pad(i() + 1)} class="row--discover">
                  <For each={r.cards}>{(c, j) => <DiscoverTile card={c} onOpen={sheet.open} autofocus={i() === 0 && j() === 0} />}</For>
                </Row>
              )}
            </For>
          </div>
        </Show>
      </Show>
      {sheet.view()}
    </div>
  );
}

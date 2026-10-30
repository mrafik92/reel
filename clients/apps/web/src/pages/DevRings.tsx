import { For, onCleanup, type JSX } from "solid-js";
import type { RequestState } from "@reel/sdk";
import { PosterCard, ProgressRing, RING_LABEL, createDownloadProgressStore, type ItemSummary } from "@reel/ui";
import { createMockProgressFeed } from "../lib/mockEvents";
import { posterArt } from "../api/mock/art";
import { usePageFocus } from "../lib/focus";

interface Demo {
  tmdb_id: number;
  title: string;
  year: number;
  state: RequestState;
  progress?: number;
  script?: "live" | "stall" | "static";
  note: string;
}

const DEMOS: Demo[] = [
  { tmdb_id: 9001, title: "Dune Sea", year: 2026, state: "requested", script: "live", note: "Live: full lifecycle" },
  { tmdb_id: 9002, title: "Comet Season", year: 2025, state: "downloading", progress: 0.2, script: "stall", note: "Live: stalls, then recovers" },
  { tmdb_id: 9003, title: "Requested", year: 2024, state: "requested", note: "requested / approved" },
  { tmdb_id: 9004, title: "Searching", year: 2024, state: "searching", note: "searching — spinner" },
  { tmdb_id: 9005, title: "In the Queue", year: 2023, state: "queued", note: "queued — spinner" },
  { tmdb_id: 9006, title: "Downloading", year: 2023, state: "downloading", progress: 0.37, note: "downloading — %" },
  { tmdb_id: 9007, title: "Stalled Out", year: 2022, state: "stalled", progress: 0.62, note: "stalled — amber" },
  { tmdb_id: 9008, title: "Importing", year: 2022, state: "importing", note: "importing — spinner" },
  { tmdb_id: 9009, title: "Failed Grab", year: 2021, state: "failed", progress: 0.4, note: "failed — retry" },
  { tmdb_id: 9010, title: "Not Found", year: 2020, state: "not_found", note: "not_found — retry" },
  { tmdb_id: 9011, title: "Ready to Watch", year: 2020, state: "available", progress: 1, note: "available → Play" },
  { tmdb_id: 9012, title: "Declined", year: 2019, state: "declined", note: "declined — no ring" },
];

const SPECIMENS: { state: RequestState; progress?: number }[] = [
  { state: "requested" },
  { state: "searching" },
  { state: "queued" },
  { state: "downloading", progress: 0.08 },
  { state: "downloading", progress: 0.64 },
  { state: "stalled", progress: 0.41 },
  { state: "importing" },
  { state: "available", progress: 1 },
  { state: "failed" },
  { state: "not_found" },
];

export default function DevRings(): JSX.Element {
  const feed = createMockProgressFeed(DEMOS);
  const store = createDownloadProgressStore(feed.source);
  onCleanup(() => feed.stop());

  const items: (ItemSummary & { note: string; tmdb: number })[] = DEMOS.map((d, i) => ({
    id: 90000 + i,
    kind: "movie",
    title: d.title,
    year: d.year,
    poster_url: posterArt(d.title, 7000 + i * 37),
    played: false,
    added_at: Math.floor(Date.now() / 1000),
    note: d.note,
    tmdb: d.tmdb_id,
  }));

  usePageFocus(() => true);

  return (
    <div class="page lab">
      <header class="page-head">
        <p class="eyebrow eyebrow--accent">Design lab · SPEC §7.1</p>
        <h1 class="page-title">Download progress ring</h1>
        <p class="lab__lede">
          Every state a requested title can be in, driven through <code>createDownloadProgressStore</code> by a
          simulated <code>request_progress</code> feed. The first two cards run live; failed cards retry on Enter.
        </p>
      </header>

      <section class="lab__section">
        <h2 class="section-title">On posters</h2>
        <div class="results-grid lab__grid" data-nav-group="free">
          <For each={items}>
            {(it) => (
              <div class="lab__cell">
                <PosterCard
                  item={it}
                  href="#"
                  onClick={(e) => e.preventDefault()}
                  download={store.get(it.tmdb)}
                  onRetry={() => feed.retry(it.tmdb)}
                  subtitle={it.note}
                />
              </div>
            )}
          </For>
        </div>
      </section>

      <section class="lab__section">
        <h2 class="section-title">Specimens</h2>
        <div class="lab__specimens">
          <For each={SPECIMENS}>
            {(s) => (
              <figure class="lab__specimen" tabindex="0" data-focusable>
                <div class="lab__sizes">
                  <ProgressRing state={s.state} progress={s.progress} size="2.5rem" />
                  <ProgressRing state={s.state} progress={s.progress} size="4.5rem" />
                  <ProgressRing state={s.state} progress={s.progress} size="7rem" />
                </div>
                <figcaption>
                  <strong>{RING_LABEL[s.state]}</strong>
                  <code>{s.state}{s.progress != null ? ` · ${Math.round(s.progress * 100)}%` : ""}</code>
                </figcaption>
              </figure>
            )}
          </For>
        </div>
      </section>
    </div>
  );
}

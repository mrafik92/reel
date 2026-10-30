import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js";
import { formatBytes, formatEta, formatSpeed, Icon, Img, ProgressRing, RING_LABEL } from "@reel/ui";
import type { DownloadItem } from "../api";
import { aggregate, downloadQueue, refreshQueue } from "../lib/downloads";
import { usePageFocus } from "../lib/focus";
import { href } from "../lib/mode";
import { EmptyState } from "../components/States";

interface SeriesGroup {
  key: string;
  title: string;
  poster?: string | null;
  itemId?: number | null;
  items: DownloadItem[];
}

const ORDER: Partial<Record<DownloadItem["state"], number>> = { downloading: 0, importing: 1, queued: 2, stalled: 3, failed: 4 };
const ord = (s: DownloadItem["state"]) => ORDER[s] ?? 5;
const byState = (a: DownloadItem, b: DownloadItem) =>
  ord(a.state) - ord(b.state) || (a.season ?? 0) - (b.season ?? 0) || (a.episode ?? 0) - (b.episode ?? 0);

function epCode(d: DownloadItem): string {
  if (d.season == null && d.episode == null) return "";
  return `S${d.season ?? "?"} · E${d.episode ?? "?"}`;
}

/** Everything Radarr/Sonarr are fetching right now, live. */
export default function Downloads(): JSX.Element {
  // Wait for the first snapshot so focus lands on a row, not the empty state.
  const [loaded, setLoaded] = createSignal(downloadQueue().length > 0);
  onMount(() => void refreshQueue().then(() => setLoaded(true)));
  usePageFocus(loaded);

  const movies = createMemo(() => downloadQueue().filter((d) => d.media_type === "movie").sort(byState));
  const series = createMemo<SeriesGroup[]>(() => {
    const m = new Map<string, SeriesGroup>();
    for (const d of downloadQueue()) {
      if (d.media_type !== "tv") continue;
      const key = String(d.tmdb_id ?? d.tvdb_id ?? d.title);
      let g = m.get(key);
      if (!g) m.set(key, (g = { key, title: d.title, poster: d.poster_url, itemId: d.library_item_id, items: [] }));
      g.items.push(d);
    }
    const out = Array.from(m.values());
    out.forEach((g) => g.items.sort(byState));
    return out.sort((a, b) => ord(a.items[0]!.state) - ord(b.items[0]!.state));
  });
  const summary = () => {
    const q = downloadQueue();
    const n = (s: DownloadItem["state"]) => q.filter((d) => d.state === s).length;
    const speed = q.reduce((t, d) => t + (d.speed_bps ?? 0), 0);
    return [
      n("downloading") ? `${n("downloading")} downloading` : "",
      n("queued") ? `${n("queued")} queued` : "",
      n("importing") ? `${n("importing")} importing` : "",
      n("stalled") ? `${n("stalled")} stalled` : "",
      n("failed") ? `${n("failed")} failed` : "",
      speed ? formatSpeed(speed) + " total" : "",
    ].filter(Boolean).join(" · ");
  };

  return (
    <div class="page downloads">
      <header class="page-head">
        <p class="eyebrow eyebrow--accent">Radarr &amp; Sonarr</p>
        <h1 class="page-title">Downloads</h1>
        <Show when={downloadQueue().length}><p class="page-lede">{summary()}</p></Show>
      </header>

      <Show when={loaded()}>
      <Show
        when={downloadQueue().length}
        fallback={
          <EmptyState title="Nothing downloading" body="When Radarr or Sonarr grab something — from a request or on their own — it shows up here live.">
            <a class="btn btn--primary" href={href("/discover")} data-autofocus><Icon name="compass" /> <span>Find something to add</span></a>
          </EmptyState>
        }
      >
        <Show when={movies().length}>
          <section class="dl-section" data-nav-anchor>
            <h2 class="section-title">Films <span class="section-title__aside">{movies().length}</span></h2>
            <ul class="dl-list">
              <For each={movies()}>{(d) => <DownloadRow d={d} withPoster />}</For>
            </ul>
          </section>
        </Show>
        <Show when={series().length}>
          <section class="dl-section" data-nav-anchor>
            <h2 class="section-title">Series <span class="section-title__aside">{series().reduce((t, g) => t + g.items.length, 0)} episodes</span></h2>
            <For each={series()}>
              {(g) => {
                const agg = () => aggregate(g.items)!;
                return (
                  <div class="dl-group">
                    <div class="dl-group__head">
                      <Img class="dl-poster" src={g.poster} width={185} fallback={<div class="card__fallback"><span>{g.title}</span></div>} />
                      <div class="dl-group__title">
                        <strong>{g.title}</strong>
                        <span>
                          {g.items.length} {g.items.length === 1 ? "episode" : "episodes"} · {Math.floor(agg().progress * 100)}% overall
                          {agg().speed_bps ? ` · ${formatSpeed(agg().speed_bps!)}` : ""}
                        </span>
                      </div>
                      <ProgressRing state={agg().state} progress={agg().progress} size="3.2rem" />
                    </div>
                    <ul class="dl-list dl-list--nested">
                      <For each={g.items}>{(d) => <DownloadRow d={d} />}</For>
                    </ul>
                  </div>
                );
              }}
            </For>
          </section>
        </Show>
      </Show>
      </Show>
    </div>
  );
}

function DownloadRow(props: { d: DownloadItem; withPoster?: boolean }): JSX.Element {
  const d = () => props.d;
  const done = () => (d().size_bytes ? d().size_bytes! - (d().sizeleft_bytes ?? d().size_bytes! * (1 - d().progress)) : 0);
  const stats = () =>
    [
      d().state === "downloading" || d().state === "stalled" ? `${Math.floor(d().progress * 100)}%` : "",
      d().speed_bps ? formatSpeed(d().speed_bps!) : "",
      d().state === "downloading" && d().eta_seconds ? formatEta(d().eta_seconds!) : "",
      d().size_bytes ? `${formatBytes(done())} of ${formatBytes(d().size_bytes!)}` : "",
    ].filter(Boolean).join(" · ");
  const body = (): JSX.Element => (
    <>
      <Show when={props.withPoster}>
        <Img class="dl-poster" src={d().poster_url} width={185} fallback={<div class="card__fallback"><span>{d().title}</span></div>} />
      </Show>
      <div class="dl-row__main">
        <strong class="dl-row__title">
          {d().media_type === "tv" ? [epCode(d()), d().episode_title].filter(Boolean).join(" · ") || d().title : d().title}
        </strong>
        <Show when={d().release_title}><span class="dl-row__release">{d().release_title}</span></Show>
        <div class="dl-bar" aria-hidden="true"><i style={{ width: `${Math.round(d().progress * 1000) / 10}%` }} /></div>
        <span class="dl-row__stats">{stats()}{stats() ? " · " : ""}{d().arr_name}</span>
        <Show when={d().error}><span class="dl-row__error"><Icon name="alert" /> {d().error}</span></Show>
      </div>
      <div class={`dl-row__state dl-row__state--${d().state}`}>
        <ProgressRing state={d().state} progress={d().progress} size="2.8rem" />
        <span>{RING_LABEL[d().state]}</span>
      </div>
    </>
  );
  const label = () => `${d().title} ${epCode(d())}: ${RING_LABEL[d().state]} ${Math.floor(d().progress * 100)}%`;
  return (
    <li class={`dl-row dl-row--${d().state}`}>
      <Show
        when={d().library_item_id}
        fallback={<div class="dl-row__inner" tabindex="0" data-focusable aria-label={label()}>{body()}</div>}
      >
        <a class="dl-row__inner" href={href(`/item/${d().library_item_id}`)} aria-label={label()}>{body()}</a>
      </Show>
    </li>
  );
}

import { startupTiming } from "../lib/startupTiming";
import { createEffect, createResource, onCleanup, Show, type JSX } from "solid-js";
import { useParams, useSearchParams } from "@solidjs/router";
import type { ItemDetail } from "@reel/ui";
import { invalidateVod, vod, type VodId } from "../api/vod";
import { decideProfile } from "../lib/playback";
import { ErrorState } from "../components/States";
import { vodPlayError } from "../components/vod/busy";
import { vodHref } from "../components/vod/links";
import { PlayerView, type PlaySource } from "./Player";

/**
 * IPTV movie / episode in the regular player: a stand-in item (title, runtime, progress) plus
 * `POST /vod/play` instead of the library's decide. Progress and stop use the usual endpoints.
 */
export default function VodPlayer(): JSX.Element {
  let timing = startupTiming("vod");
  onCleanup(() => timing?.finish("canceled"));
  const params = useParams<{ kind: string; id: string }>();
  const [search] = useSearchParams<{ series?: string }>();
  const kind = () => (params.kind === "episode" ? "episode" : "movie");

  const [target, { refetch }] = createResource(
    () => [kind(), params.id, search.series] as const,
    async ([k, id, seriesId]) => (k === "movie" ? movieTarget(id) : episodeTarget(id, seriesId)),
  );

  createEffect(() => { if (target.error) timing?.finish("failed"); });

  return (
    <div class="player">
      <Show when={!target.error} fallback={<ErrorState error={target.error} title="Couldn't open this IPTV title" onRetry={() => { timing = startupTiming("vod"); void refetch(); }} />}>
        <Show when={target()} keyed fallback={<div class="player__loading"><span class="spinner" /></div>}>
          {(t) => <PlayerView item={t.item} source={t.source} startup={timing} />}
        </Show>
      </Show>
    </div>
  );
}

interface Target {
  item: ItemDetail;
  source: PlaySource;
}

function hash(str: string): number {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** Track choices are remembered per item id: keep IPTV ids clear of library ids. */
function synthId(kind: string, id: VodId): number {
  const n = typeof id === "number" ? id : /^\d+$/.test(id) ? Number(id) : hash(String(id)) % 1e9;
  return -((kind === "movie" ? 1e9 : 2e9) + n);
}

function source(kind: "movie" | "episode", id: VodId, back: string, eyebrow?: string): PlaySource {
  // Admins answering a 409 "in use" with Take over: the next start asks the server to stop theirs.
  let takeover = false;
  return {
    decide: (o) => {
      const t = takeover;
      takeover = false;
      return vod.play({
        kind,
        id,
        ...decideProfile(),
        audio_index: o.audio,
        subtitle_index: o.subtitle >= 0 ? o.subtitle : null,
        start_s: o.start > 0 ? Math.floor(o.start) : null,
        ...(o.quality ? { quality: o.quality } : {}),
        ...(t ? { takeover: true } : {}),
      });
    },
    error: (e) => vodPlayError(e, { takeover: () => (takeover = true) }),
    eyebrow,
    back,
    onClose: invalidateVod,
  };
}

function stand(o: Partial<ItemDetail> & { id: number; title: string; kind: ItemDetail["kind"]; runtime_s?: number | null; container?: string | null }): ItemDetail {
  return {
    played: false,
    added_at: 0,
    genres: [],
    cast: [],
    children_count: 0,
    ...o,
    parent_id: null,
    files: [{ id: 0, container: o.container ?? "ts", duration_s: o.runtime_s ?? 0, size: 0, url: "", audio: [], subtitles: [] }],
  };
}

async function movieTarget(id: string): Promise<Target> {
  const m = await vod.movie(id);
  return {
    item: stand({
      id: synthId("movie", m.id), kind: "movie", title: m.title, year: m.year, poster_url: m.poster_url, backdrop_url: m.backdrop_url,
      runtime_s: m.runtime_s, progress: m.progress, played: !!m.played, container: m.container,
    }),
    source: source("movie", m.id, vodHref({ kind: "movie", id: m.id }), "IPTV"),
  };
}

async function episodeTarget(id: string, seriesId?: string): Promise<Target> {
  if (!seriesId) throw new Error("This episode link is missing its series.");
  const s = await vod.series(seriesId);
  for (const season of s.seasons) {
    const ep = season.episodes.find((e) => String(e.id) === id);
    if (!ep) continue;
    const generic = /^(episode|ep\.?|الحلقة)\s*\d+$/i.test(ep.title.trim());
    return {
      item: stand({
        id: synthId("episode", ep.id), kind: "episode", title: generic ? `Episode ${ep.episode_no}` : ep.title,
        season_no: season.season_no, episode_no: ep.episode_no, backdrop_url: ep.still_url ?? s.backdrop_url,
        runtime_s: ep.runtime_s, progress: ep.progress, played: !!ep.played,
      }),
      source: source("episode", ep.id, vodHref({ kind: "series", id: s.id }), `${s.title} · S${season.season_no} E${ep.episode_no}`),
    };
  }
  throw new Error("This episode isn't on your IPTV provider anymore.");
}

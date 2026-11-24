import { createSignal } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { api, ApiError, invalidate, type DiscoverCard, type MediaType, type ItemSummary } from "../api";
import { downloads, noteRequest } from "./downloads";
import { toast } from "./toast";
import { itemHref, playHref, toPath } from "./links";

/** Find the library item behind an available TMDB card (the card has no item id). */
export async function findLibraryItem(c: { title: string; year?: number | null; media_type: MediaType; tmdb_id: number }): Promise<ItemSummary | undefined> {
  const kind = c.media_type === "tv" ? "show" : "movie";
  const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
  const cands = (await api.search(c.title, 20)).filter((r) => r.kind === kind);
  cands.sort((a, b) => score(b) - score(a));
  function score(r: ItemSummary): number {
    return (norm(r.title) === norm(c.title) ? 2 : 0) + (c.year && r.year === c.year ? 1 : 0);
  }
  for (const cand of cands.slice(0, 4)) {
    try {
      const d = await api.item(cand.id);
      if (d.tmdb_id === c.tmdb_id) return cand;
    } catch {
      /* keep looking */
    }
  }
  return cands.find((r) => score(r) === 3);
}

const [busy, setBusy] = createSignal<Record<string, boolean>>({});
const key = (m: MediaType, id: number) => `${m}:${id}`;
export const isBusy = (m: MediaType, id: number) => !!busy()[key(m, id)];

/** Add / Play / Retry actions shared by Discover rows, search, the detail sheet and My requests. */
export function useRequestActions() {
  const navigate = useNavigate();

  async function add(c: Pick<DiscoverCard, "media_type" | "tmdb_id" | "title">, seasons?: number[]): Promise<boolean> {
    const k = key(c.media_type, c.tmdb_id);
    if (busy()[k]) return false;
    setBusy((b) => ({ ...b, [k]: true }));
    try {
      const r = await api.createRequest({ media_type: c.media_type, tmdb_id: c.tmdb_id, seasons: seasons && seasons.length ? seasons : undefined });
      noteRequest(r);
      invalidate("discover");
      invalidate("requests");
      toast(r.status === "requested" ? `Requested “${c.title}” — waiting for approval` : `Adding “${c.title}”`, "success");
      return true;
    } catch (e) {
      toast(requestError(e, c.title), "error", 6000);
      return false;
    } finally {
      setBusy((b) => {
        const n = { ...b };
        delete n[k];
        return n;
      });
    }
  }

  async function retry(requestId: number | string, title?: string): Promise<void> {
    try {
      await retrySearch(requestId);
      toast(`Searching again${title ? ` for “${title}”` : ""}`);
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't retry", "error");
    }
  }

  async function retryAll(requestIds: number[]): Promise<void> {
    let failed = 0;
    for (const id of requestIds) {
      try {
        await retrySearch(id);
      } catch {
        failed += 1;
      }
    }
    const started = requestIds.length - failed;
    const summary = `Started ${started} ${started === 1 ? "retry" : "retries"}`;
    toast(failed ? `${summary}. Couldn't retry ${failed}.` : summary, failed ? "error" : "success");
  }

  async function play(c: { title: string; year?: number | null; media_type: MediaType; tmdb_id: number; library_item_id?: number | null }): Promise<void> {
    // Cards carry the library movie/show id when available — no lookup needed.
    if (c.library_item_id) {
      const ref = { id: c.library_item_id, kind: c.media_type === "tv" ? ("show" as const) : ("movie" as const) };
      navigate(toPath(ref.kind === "movie" ? playHref(ref) : itemHref(ref)));
      return;
    }
    try {
      // Older servers / just-imported titles: find it by title + tmdb_id.
      const it = await findLibraryItem(c);
      if (!it) {
        toast(`“${c.title}” is still being added to your library — try again in a moment`);
        invalidate("search");
        return;
      }
      navigate(toPath(it.kind === "movie" ? playHref(it) : itemHref(it)));
    } catch (e) {
      toast(e instanceof Error ? e.message : "Couldn't open that title", "error");
    }
  }

  return { add, retry, retryAll, play, progressFor: (tmdbId: number) => downloads.get(tmdbId) };
}

async function retrySearch(requestId: number | string): Promise<void> {
  const request = await api.retryRequest(Number(requestId));
  noteRequest(request);
  invalidate("requests");
}

export function requestError(e: unknown, title: string): string {
  if (e instanceof ApiError) {
    if (e.status === 409) return `“${title}” is already in your library or requested`;
    if (e.status === 424) return "Requests aren't set up yet — an admin needs to connect Radarr/Sonarr";
    if (e.status === 429) return "You've reached your request limit for now";
    return e.message;
  }
  return "Couldn't add that title";
}

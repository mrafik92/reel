import type { ItemSummary, Library } from "@reel/ui";
import { href } from "./mode";

export function itemHref(it: Pick<ItemSummary, "id" | "kind" | "parent_id">): string {
  return href(`/item/${it.id}`);
}

export function playHref(
  it: Pick<ItemSummary, "id">,
  opts: { file?: number; fromStart?: boolean; audio?: number | null; sub?: number | null; cast?: string } = {},
): string {
  const q: string[] = [];
  if (opts.file != null) q.push(`file=${opts.file}`);
  if (opts.fromStart) q.push("start=0");
  // Track choices made on the detail page travel with the link (-1 = subtitles off).
  if (opts.audio != null) q.push(`audio=${opts.audio}`);
  if (opts.sub != null) q.push(`sub=${opts.sub}`);
  // "Play on…": the player starts on that target ("chromecast", "airplay", "dlna:<id>").
  if (opts.cast) q.push(`cast=${encodeURIComponent(opts.cast)}`);
  return href(`/play/${it.id}${q.length ? "?" + q.join("&") : ""}`);
}

export function libraryHref(lib: Pick<Library, "id" | "kind">): string {
  return href(`/${lib.kind === "shows" ? "shows" : "movies"}?lib=${lib.id}`);
}

/** Turn an href from these helpers into a router path (strips the hash-routing "#"). */
export function toPath(h: string): string {
  return h.charAt(0) === "#" ? h.slice(1) : h;
}

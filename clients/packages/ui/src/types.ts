/**
 * Phase 1 API response shapes (SPEC §4.1). Wire format is snake_case.
 *
 * Hand-written until the server exports these through OpenAPI; once they land in
 * `@reel/sdk` (`components["schemas"]`), re-export from there instead.
 */

/** Database ids are SQLite INTEGER primary keys. */
export type Id = number;

export type LibraryKind = "movies" | "shows";
export type ItemKind = "movie" | "show" | "season" | "episode";
export type Hdr = "none" | "hdr10" | "hlg" | "dv";

export interface Library {
  id: Id;
  name: string;
  kind: LibraryKind;
  paths: string[];
  item_count: number;
}

export interface ItemSummary {
  id: Id;
  kind: ItemKind;
  title: string;
  year?: number | null;
  /** Relative, e.g. `/api/v1/images/poster/{item_id}?w=342` */
  poster_url?: string | null;
  backdrop_url?: string | null;
  season_no?: number | null;
  episode_no?: number | null;
  parent_id?: Id | null;
  /** 0..1 */
  progress?: number | null;
  played: boolean;
  /** Unix seconds */
  added_at: number;
}

export interface CastMember {
  name: string;
  character?: string | null;
  photo_url?: string | null;
}

export interface VideoStreamInfo {
  codec: string;
  width: number;
  height: number;
  hdr: Hdr;
  bit_depth: number;
}

export interface AudioStreamInfo {
  index: number;
  codec: string;
  channels: number;
  language?: string | null;
  title?: string | null;
  default: boolean;
}

export interface SubtitleStreamInfo {
  index: number;
  codec: string;
  language?: string | null;
  title?: string | null;
  forced: boolean;
  external: boolean;
}

export interface MediaFile {
  id: Id;
  container: string;
  duration_s: number;
  size: number;
  /** `/api/v1/direct/{id}` */
  url: string;
  /**
   * Multi-episode files (`S01E02E03`): every episode item stored in this file, in
   * episode order. Absent for ordinary files.
   */
  item_ids?: Id[] | null;
  video?: VideoStreamInfo | null;
  audio: AudioStreamInfo[];
  subtitles: SubtitleStreamInfo[];
}

export interface ItemDetail extends ItemSummary {
  overview?: string | null;
  runtime_s?: number | null;
  rating?: number | null;
  content_rating?: string | null;
  genres: string[];
  tmdb_id?: number | null;
  cast: CastMember[];
  children_count: number;
  files: MediaFile[];
}

export interface Page<T> {
  items: T[];
  next_cursor?: string | null;
}

export interface HomeRow {
  library_id: Id;
  title: string;
  items: ItemSummary[];
}

export interface Home {
  continue_watching: ItemSummary[];
  recently_added: ItemSummary[];
  rows: HomeRow[];
}

export type LibrarySort = "title" | "added" | "year";

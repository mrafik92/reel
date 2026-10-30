/**
 * Wire types, generated from `server/openapi.json` (`bun run gen:api` in apps/web).
 * Switch these imports to `@reel/sdk` once its schema is regenerated.
 */
import type { components } from "./openapi";
import type { ItemSummary } from "@reel/ui";

type S = components["schemas"];

export type Me = S["Me"];
export type Role = S["RoleDto"];
export type Limits = S["LimitsDto"];
export type LoginResponse = S["LoginResponse"];
export type SetupStatus = S["SetupStatus"];
export type InviteInfo = S["InviteInfoDto"];

export type MediaType = S["MediaTypeDto"];
export type DiscoverCard = S["DiscoverCardDto"];
export type DiscoverPage = S["DiscoverPageDto"];
export type DiscoverDetail = S["DiscoverDetailDto"];
export type DiscoverList = "trending" | "popular" | "upcoming";
export type RequestDto = S["RequestDto"];
export type RequestState = S["RequestState"];
export type ProgressDto = S["ProgressDto"];
export type CreateRequest = S["CreateRequestBody"];

export type DecideRequest = S["DecideRequest"];
export type DecideResponse = S["DecideResponse"];
export type DeviceProfileWire = S["DeviceProfile"];
export type PlaybackMode = S["PlaybackMode"];
export type PlaybackDecision = S["PlaybackDecision"];
export type SubtitleOption = S["SubtitleOption"];

export type UserDto = S["UserDto"];
export type CreateUser = S["CreateUserBody"];
export type PatchUser = S["PatchUserBody"];
export type InviteDto = S["InviteDto"];
export type CreateInvite = S["CreateInviteBody"];
export type ArrServer = S["ArrServerDto"];
export type CreateArrServer = S["CreateArrServer"];
export type UpdateArrServer = S["UpdateArrServer"];
export type ArrProfiles = S["ArrProfiles"];
export type DownloadClient = S["DownloadClientDto"];
export type CreateDownloadClient = S["CreateDownloadClient"];
export type NotificationTarget = S["NotificationDto"];
export type CreateNotification = S["CreateNotification"];
export type AdminSession = S["AdminSession"];
export type HwAccelStatus = S["HwAccelStatus"];
export type HwAccel = S["HwAccel"];
export type CreateLibrary = S["CreateLibrary"];
export type SessionDto = S["SessionDto"];

// ---------------------------------------------------------------------------
// People, reviews, downloads (server round 4).

export type CastCredit = S["CastMember"];
/** Discover detail; `cast` entries without `person_tmdb_id` are local (.nfo) only. */
export type DiscoverDetailWithCast = DiscoverDetail;
/** Discover card shape (incl. `library_item_id`) + role. `role`: "cast" | "crew". */
export type PersonCredit = S["PersonCreditDto"];
export type PersonDetail = S["PersonDto"];

/** Known values; the wire type is a plain string, so UI code keeps a default branch. */
export type RatingSource = "imdb" | "rotten_tomatoes" | "metacritic" | "tmdb";
export type ReviewSource = "tmdb" | "trakt";
export type LinkSource = "imdb" | "rotten_tomatoes" | "letterboxd" | "tmdb" | "trakt";
export type Rating = S["RatingDto"];
/** `created_at` is ISO-8601; `avatar_url` only for TMDB-hosted avatars (else initials). */
export type Review = S["ReviewDto"];
export type ReviewLink = S["ReviewLinkDto"];
export type Reviews = S["ReviewsDto"];

/** queued | downloading | stalled | importing | failed (typed as RequestState on the wire). */
export type DownloadState = RequestState;
/** One Radarr/Sonarr queue entry; `id` is `"{arr_server_id}:{queue_id}"`. */
export type DownloadItem = S["DownloadDto"];
/** Pushed on change, plus a final `items: []` when the queue drains. */
export interface DownloadsEvent {
  type: "downloads";
  items: DownloadItem[];
}

// ---------------------------------------------------------------------------
// Browse / picker / unified search (server round 5). Hand-written until the
// OpenAPI export includes them; shapes follow the agreed contract.

export interface GenreCount {
  name: string;
  count: number;
}

/** Filters shared by `/libraries/{id}/items` and `/pick`. */
export interface LibraryFilters {
  /** AND-combined. */
  genre?: string[];
  year_from?: number;
  year_to?: number;
  min_rating?: number;
  max_runtime_min?: number;
  unwatched?: boolean;
}

export type LaneKind = "because_you_watched" | "top_unwatched" | "short" | "new" | "genre" | "rediscover";

export interface BrowseLane {
  key: string;
  title: string;
  subtitle?: string | null;
  kind: LaneKind | string;
  items: ItemSummary[];
  /** `query` is the library-items query (object or `a=b&c=d` string). */
  see_all?: { library_id: number; query: Record<string, string | number | boolean | null> | string } | null;
}

export interface PersonHit {
  tmdb_id: number;
  name: string;
  photo_url?: string | null;
  known_for_department?: string | null;
  known_for: string[];
  in_library_count: number;
}

export interface SearchAll {
  library: ItemSummary[];
  episodes: ItemSummary[];
  people: PersonHit[];
  discover: DiscoverCard[];
}

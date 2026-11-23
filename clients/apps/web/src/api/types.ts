/**
 * Wire types, generated from `server/openapi.json` (`bun run gen:api` in apps/web).
 * Switch these imports to `@reel/sdk` once its schema is regenerated.
 */
import type { components } from "./openapi";

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

/**
 * Playback quality (#87): `auto` | `original` | a ladder rung. In `server/openapi.json` since #48;
 * added by hand here because a full `gen:api` regeneration pulls in unrelated schema drift.
 */
export type Quality = "auto" | "original" | "1080p" | "720p" | "480p";
export interface QualityOption {
  id: Quality;
  label: string;
  /** Expected total bitrate (kbit/s) of this choice. */
  bitrate_kbps: number;
  /** False when it would upscale, change nothing, or exceed the user's bitrate limit. */
  available: boolean;
}
/** One variant of an HLS session's master playlist (#111), in master order (ascending bandwidth). */
export interface VariantOption {
  /** `original` or a rung; also the variant's URL path segment (`…/{id}/index.m3u8`). */
  id: Quality;
  label: string;
  width: number;
  height: number;
  /** Peak bitrate (kbit/s): the master's `BANDWIDTH`. */
  bandwidth_kbps: number;
  /** Average bitrate (kbit/s): the master's `AVERAGE-BANDWIDTH`. */
  average_kbps: number;
  codecs?: string;
  /** The picture is re-encoded. */
  transcode: boolean;
}
export type DecideRequest = S["DecideRequest"] & {
  quality?: Quality | null;
  /** Client bandwidth hint; the server caps it by the user's limit. */
  max_bitrate_kbps?: number | null;
  for_cast?: boolean | null;
  /** The player switches variants itself (hls.js): ask for a multi-variant master (#111). */
  adaptive?: boolean | null;
  /** Measured throughput (kbit/s): picks the start variant; caps nothing. */
  bandwidth_kbps?: number | null;
};
/** Older servers (and some mocks) omit the quality fields: treat them as optional. */
export type DecideResponse = S["DecideResponse"] & {
  /** Resolved: `original` or a rung, never `auto`. */
  quality?: Quality;
  bitrate_kbps?: number;
  quality_options?: QualityOption[];
  /** The master playlist's variants (several for an adaptive session, #111). */
  variants?: VariantOption[];
  /** Adaptive sessions: the variant to start on (the server prestarted it). */
  start_variant?: Quality;
};
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
// Browse / picker / unified search (server round 5).

export type GenreCount = S["GenreCount"];
export type LaneKind = S["LaneKind"];
/** `see_all` only where meaningful; `query` is a query string for `/libraries/{id}/items`. */
export type BrowseLane = S["Lane"];
/** Only people with a TMDB id are returned. */
export type PersonHit = S["PersonHit"];
export type SearchAll = S["SearchAll"];

/** Filters shared by `/libraries/{id}/items` and `/pick` (invalid values → 400). */
export interface LibraryFilters {
  /** AND-combined. */
  genre?: string[];
  year_from?: number;
  year_to?: number;
  min_rating?: number;
  max_runtime_min?: number;
  unwatched?: boolean;
}

// ---- second factor (#146) ----------------------------------------------------------------------
/** `POST /auth/login` answers with this (202) when the account has a second factor. */
export interface MfaChallenge {
  mfa_token: string;
  methods: string[];
  expires_in_s: number;
}
export interface MfaLogin {
  mfa_token: string;
  code?: string;
  recovery_code?: string;
  passkey?: unknown;
}
export interface PasskeyInfo {
  id: number;
  name: string;
  created_at: number;
  last_used_at?: number | null;
}
export interface AccountSecurity {
  totp: boolean;
  passkeys: PasskeyInfo[];
  recovery_codes_left: number;
  passkeys_available: boolean;
  must_enroll: boolean;
}
export interface TotpEnrollment {
  secret: string;
  otpauth_uri: string;
}
export interface RecoveryCodes {
  recovery_codes?: string[] | null;
}
export interface DeviceSession {
  id: number;
  device_id?: string | null;
  device_name?: string | null;
  client?: string | null;
  ip?: string | null;
  last_seen: number;
  expires_at?: number | null;
  current: boolean;
}

// ---- TV sign-in by code (#147) ------------------------------------------------------------------
export interface DevicePairing {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete: string;
  expires_in_s: number;
  interval_s: number;
}
export interface DeviceLinkInfo {
  device_name?: string | null;
  client?: string | null;
  ip?: string | null;
  expires_in_s: number;
}

// ---- provider sign-in (#149) --------------------------------------------------------------------
export interface OidcProviderPublic {
  slug: string;
  name: string;
}
export interface OidcProviderAdmin extends OidcProviderPublic {
  id: number;
  issuer: string;
  client_id: string;
  secret_set: boolean;
  enabled: boolean;
  redirect_uri?: string | null;
}
export interface CreateOidcProvider {
  preset: string;
  name?: string;
  issuer?: string;
  client_id: string;
  client_secret: string;
}
export interface PatchOidcProvider {
  name?: string;
  client_id?: string;
  client_secret?: string;
  enabled?: boolean;
}
export interface LinkedIdentity {
  id: number;
  provider: string;
  label?: string | null;
  created_at: number;
  last_used_at?: number | null;
}

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

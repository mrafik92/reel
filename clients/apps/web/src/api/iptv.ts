/**
 * Live TV (IPTV) endpoints — `/api/v1/iptv/*` and `/api/v1/admin/iptv/*`.
 *
 * Kept separate from the main `Api` so the Live TV lane stays self-contained. Same rules:
 * HTTP by default, in-browser fixtures with `?mock=1` (code-split out of production).
 */
import type { components } from "./openapi";
import { MOCK } from "../lib/mode";
import { http, request } from "./http";

type S = components["schemas"];

export type IptvChannel = S["IptvChannel"];
export type IptvGroup = S["IptvGroup"];
export type Program = S["Program"];
export type NowNext = S["NowNext"];
export type EpgGrid = S["EpgGrid"];
export type EpgChannel = S["EpgChannel"];
export type LiveMode = S["LiveModeDto"];
export type TuneBody = S["TuneBody"];
export type TuneResponse = S["TuneResponse"];
export type SourceDto = S["SourceDto"];
export type SourceBody = S["SourceBody"];
export type SourcePatch = S["SourcePatch"];
export type SourceTest = S["SourceTest"];
export type AccountDto = S["AccountDto"];
export type AdminChannel = S["AdminChannel"];
export type ChannelPatch = S["ChannelPatchBody"];
export type AliasDto = S["AliasDto"];
export type HubStatus = S["HubStatus"];
export type ChannelMetrics = S["ChannelMetricsDto"];
export type LiveSession = S["LiveSessionDto"];
export type SourceUsage = S["SourceUsage"];

export interface EpgQuery {
  from: number;
  to: number;
  channels: number[];
  detail?: boolean;
}

export interface IptvApi {
  channels(o?: { group?: string; favorites?: boolean; q?: string }): Promise<IptvChannel[]>;
  groups(): Promise<IptvGroup[]>;
  epg(q: EpgQuery): Promise<EpgGrid>;
  now(channels?: number[]): Promise<NowNext[]>;
  favorite(id: number, on: boolean): Promise<void>;
  tune(id: number, body: TuneBody): Promise<TuneResponse>;
  stop(sessionId: string, keepalive?: boolean): Promise<void>;

  // Admin
  sources(): Promise<SourceDto[]>;
  source(id: number): Promise<SourceDto>;
  createSource(b: SourceBody): Promise<SourceDto>;
  updateSource(id: number, b: SourcePatch): Promise<SourceDto>;
  deleteSource(id: number): Promise<void>;
  refreshSource(id: number): Promise<void>;
  testSource(id: number): Promise<SourceTest>;
  testConfig(b: SourceBody): Promise<SourceTest>;
  adminChannels(o?: { source?: number; q?: string; group?: string; hidden?: boolean }): Promise<AdminChannel[]>;
  patchChannel(id: number, b: ChannelPatch): Promise<void>;
  setAliases(id: number, a: AliasDto[]): Promise<void>;
  hub(): Promise<HubStatus>;
  liveSessions(): Promise<LiveSession[]>;
}

const ids = (l?: number[]) => (l && l.length ? l.join(",") : undefined);

const httpIptv: IptvApi = {
  channels: (o = {}) => http.get("/iptv/channels", { group: o.group, favorites: o.favorites || undefined, q: o.q }),
  groups: () => http.get("/iptv/groups"),
  epg: (q) => http.get("/iptv/epg", { from: q.from, to: q.to, channels: ids(q.channels), detail: q.detail || undefined }),
  now: (channels) => http.get("/iptv/now", { channels: ids(channels) }),
  favorite: (id, on) => (on ? http.post(`/iptv/channels/${id}/favorite`) : http.del(`/iptv/channels/${id}/favorite`)),
  tune: (id, body) => http.post(`/iptv/channels/${id}/tune`, body),
  stop: (sid, keepalive) => http.del(`/iptv/sessions/${encodeURIComponent(sid)}`, { keepalive }),

  sources: () => http.get("/admin/iptv/sources"),
  source: (id) => http.get(`/admin/iptv/sources/${id}`),
  createSource: (b) => http.post("/admin/iptv/sources", b),
  updateSource: (id, b) => http.patch(`/admin/iptv/sources/${id}`, b),
  deleteSource: (id) => http.del(`/admin/iptv/sources/${id}`),
  refreshSource: (id) => http.post(`/admin/iptv/sources/${id}/refresh`),
  testSource: (id) => http.post(`/admin/iptv/sources/${id}/test`),
  testConfig: (b) => http.post("/admin/iptv/sources/test", b),
  adminChannels: (o = {}) => http.get("/admin/iptv/channels", { source: o.source, q: o.q, group: o.group, hidden: o.hidden }),
  patchChannel: (id, b) => http.patch(`/admin/iptv/channels/${id}`, b),
  setAliases: (id, a) => request("PUT", `/admin/iptv/channels/${id}/aliases`, { body: a }),
  hub: () => http.get("/admin/iptv/hub"),
  liveSessions: () => http.get("/admin/iptv/sessions"),
};


let impl: Promise<IptvApi> | undefined;
function backend(): Promise<IptvApi> {
  if (!impl) impl = MOCK ? import("./mock/iptv").then((m) => m.createMockIptv()) : Promise.resolve(httpIptv);
  return impl;
}

function wrap(): IptvApi {
  const out: Record<string, unknown> = {};
  (Object.keys(httpIptv) as (keyof IptvApi)[]).forEach((k) => {
    out[k] = (...args: unknown[]) => backend().then((a) => (a[k] as unknown as (...x: unknown[]) => Promise<unknown>)(...args));
  });
  return out as unknown as IptvApi;
}

export const iptv: IptvApi = wrap();

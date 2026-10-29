import createClient from "openapi-fetch";
import type { paths } from "./schema";

export type { paths, components, operations } from "./schema";
export * from "./events";

export function createReelClient(baseUrl: string, token?: string) {
  return createClient<paths>({
    baseUrl,
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
  });
}

export type ReelClient = ReturnType<typeof createReelClient>;

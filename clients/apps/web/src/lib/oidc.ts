/**
 * "Sign in with …" (#149). The browser keeps a random secret for the trip to the provider and
 * proves it on return, so a link someone else started can't sign this browser into their account.
 */
import { api, type MfaChallenge } from "../api";
import { deviceFields } from "./device";
import { MOCK, TV } from "./mode";

const PENDING = "reel.oidc.pending";
const INVITE = "reel.oidc.invite";

export type OidcPurpose = "login" | "invite" | "link";

export interface OidcPending {
  binding: string;
  purpose: OidcPurpose;
  provider: string;
  invite?: string;
  next?: string;
}

export interface InviteLink {
  code: string;
  ticket: string;
  binding: string;
  provider: string;
}

function store(key: string, value: unknown): void {
  try {
    if (value === null) sessionStorage.removeItem(key);
    else sessionStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* private mode: the return step reports the sign-in as expired */
  }
}

function load<T>(key: string): T | undefined {
  try {
    const v = sessionStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : undefined;
  } catch {
    return undefined;
  }
}

function b64url(bytes: Uint8Array): string {
  let s = "";
  bytes.forEach((b) => (s += String.fromCharCode(b)));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function newBinding(): Promise<{ binding: string; hash: string }> {
  const raw = new Uint8Array(32);
  crypto.getRandomValues(raw);
  const binding = b64url(raw);
  const hash = b64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(binding))));
  return { binding, hash };
}

/** TVs sign in with a code instead (#147), and hashing needs a secure context. */
export function oidcSupported(): boolean {
  return !TV && (MOCK || (typeof window !== "undefined" && window.isSecureContext && !!window.crypto?.subtle));
}

export async function startOidc(provider: { slug: string; name: string }, purpose: "login" | "invite", opts: { invite?: string; next?: string } = {}): Promise<void> {
  const { binding, hash } = await newBinding();
  const r = await api.oidcStart(provider.slug, { binding_hash: hash, purpose, ...deviceFields() });
  store(PENDING, { binding, purpose, provider: provider.name, ...opts } satisfies OidcPending);
  location.assign(r.authorization_url);
}

export async function startOidcLink(provider: { slug: string; name: string }, password: string): Promise<void> {
  const { binding, hash } = await newBinding();
  const r = await api.identityStart(provider.slug, password, hash);
  store(PENDING, { binding, purpose: "link", provider: provider.name } satisfies OidcPending);
  location.assign(r.authorization_url);
}

/** The trip in progress, once: the secret isn't kept after the return. */
export function takeOidcPending(): OidcPending | undefined {
  const p = load<OidcPending>(PENDING);
  store(PENDING, null);
  return p;
}

export const saveInviteLink = (l: InviteLink): void => store(INVITE, l);
export const clearInviteLink = (): void => store(INVITE, null);
export function inviteLink(code: string): InviteLink | undefined {
  const l = load<InviteLink>(INVITE);
  return l && l.code === code ? l : undefined;
}

// A provider login on an account with a second factor continues on the login page.
let pendingChallenge: MfaChallenge | undefined;
export const setPendingChallenge = (c: MfaChallenge): void => void (pendingChallenge = c);
export function takePendingChallenge(): MfaChallenge | undefined {
  const c = pendingChallenge;
  pendingChallenge = undefined;
  return c;
}

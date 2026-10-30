import { createSignal } from "solid-js";
import { api, ApiError, getToken, onUnauthorized, setToken, invalidate, type Credentials, type LoginResponse, type Me } from "../api";
import { deviceFields } from "./device";
import { MOCK } from "./mode";
import { needsServerSetup } from "@reel/ui";

export type AuthStatus = "checking" | "offline" | "needs_server" | "needs_setup" | "anon" | "authed";

const [status, setStatus] = createSignal<AuthStatus>("checking");
const [user, setUser] = createSignal<Me | null>(null);
const [bootError, setBootError] = createSignal<unknown>();

export { status as authStatus, user as currentUser, bootError };

export const isAdmin = () => user()?.role === "admin";
/** Admins and auto-approve users get "Add" (immediate); everyone else "Request". */
export const canAutoAdd = () => isAdmin() || !!user()?.limits?.auto_approve;

const listeners = new Set<(u: Me | null) => void>();
/** Subscribe to sign-in / sign-out (used to (re)connect the events socket). */
export function onAuthChange(fn: (u: Me | null) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
function emit(): void {
  listeners.forEach((f) => f(user()));
}

onUnauthorized(() => {
  if (status() !== "authed") return;
  signOutLocal();
});

function signOutLocal(): void {
  setToken(null);
  setUser(null);
  invalidate();
  setStatus("anon");
  emit();
}

function accept(r: LoginResponse): Me {
  setToken(r.token);
  setUser(r.user);
  invalidate();
  setStatus("authed");
  emit();
  return r.user;
}

/** Resolve who we are: valid token → authed; else first-run setup or login. */
export async function bootstrap(): Promise<void> {
  setStatus("checking");
  setBootError(undefined);
  if (!MOCK && needsServerSetup()) {
    // Packaged TV app (file://) with no server address yet.
    setStatus("needs_server");
    return;
  }
  if (MOCK) {
    setUser(await api.me());
    setStatus("authed");
    emit();
    return;
  }
  if (getToken()) {
    try {
      setUser(await api.me());
      setStatus("authed");
      emit();
      return;
    } catch (e) {
      if (!(e instanceof ApiError) || e.status !== 401) {
        setBootError(e);
        setStatus("offline");
        return;
      }
      setToken(null);
    }
  }
  try {
    const s = await api.setupStatus();
    setStatus(s.needs_setup ? "needs_setup" : "anon");
  } catch (e) {
    setBootError(e);
    setStatus("offline");
  }
}

export async function login(username: string, password: string): Promise<Me> {
  return accept(await api.login(withDevice({ username, password })));
}

export async function createAdmin(username: string, password: string): Promise<Me> {
  return accept(await api.setup(withDevice({ username, password })));
}

export async function acceptInvite(code: string, username: string, password: string): Promise<Me> {
  return accept(await api.acceptInvite(code, withDevice({ username, password })));
}

export async function logout(): Promise<void> {
  try {
    await api.logout();
  } catch {
    /* token may already be gone */
  }
  signOutLocal();
}

function withDevice(c: Credentials): Credentials {
  return { ...c, ...deviceFields() };
}

import { createSignal } from "solid-js";
import { api, ApiError, getToken, onUnauthorized, setToken, invalidate, type Credentials, type LoginResponse, type Me } from "../api";
import { deviceFields } from "./device";
import { MOCK } from "./mode";
import { needsServerSetup } from "@reel/ui";
import { autoRetry } from "./connection";

export type AuthStatus = "checking" | "offline" | "needs_server" | "needs_setup" | "anon" | "authed";

const [status, setStatus] = createSignal<AuthStatus>("checking");
const [user, setUser] = createSignal<Me | null>(null);
const [bootError, setBootError] = createSignal<unknown>();

export { status as authStatus, user as currentUser, bootError };

/** Can use admin actions now: an admin on a public address keeps viewing, but not these (#127). */
export const isAdmin = () => user()?.role === "admin" && user()?.admin_powers !== false;
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

// Couldn't reach the server at start-up (#81): keep trying in the background and whenever the tab
// comes back, instead of waiting for a refresh or the "Try again" button.
let retrying = false;
let stopRetry: (() => void) | undefined;
function retryWhileOffline(): void {
  if (stopRetry) return;
  stopRetry = autoRetry(
    () => {
      if (retrying) return;
      retrying = true;
      void bootstrap(true).finally(() => (retrying = false));
    },
    () => status() === "offline",
  );
}

/** Resolve who we are: valid token → authed; else first-run setup or login.
 *  `quiet`: a background retry; keep the offline screen up instead of flashing "checking". */
export async function bootstrap(quiet = false): Promise<void> {
  await resolve(quiet);
  if (status() === "offline") retryWhileOffline();
  else {
    stopRetry?.();
    stopRetry = undefined;
  }
}

async function resolve(quiet: boolean): Promise<void> {
  if (!quiet) setStatus("checking");
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

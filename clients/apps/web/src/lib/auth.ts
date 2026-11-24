import { createSignal } from "solid-js";
import { api, ApiError, getToken, onUnauthorized, setToken, invalidate, type Credentials, type LoginResponse, type Me, type MfaChallenge, type MfaLogin } from "../api";
import { deviceFields } from "./device";
import { MOCK } from "./mode";
import { needsServerSetup } from "@reel/ui";
import { autoRetry } from "./connection";
import { toast } from "./toast";
import { usePasskey } from "./webauthn";

export type AuthStatus = "checking" | "offline" | "needs_server" | "needs_setup" | "anon" | "authed";

const [status, setStatus] = createSignal<AuthStatus>("checking");
const [user, setUser] = createSignal<Me | null>(null);
const [bootError, setBootError] = createSignal<unknown>();

export { status as authStatus, user as currentUser, bootError };

/** Can use admin actions now: an admin on a public address keeps viewing, but not these (#127). */
export const isAdmin = () => user()?.role === "admin" && user()?.admin_powers !== false;
/** Admins and auto-approve users get "Add" (immediate); everyone else "Request". */
export const canAutoAdd = () => isAdmin() || !!user()?.limits?.auto_approve;

/** Shortest new password the server accepts (#148). It also refuses common ones. */
export const MIN_PASSWORD = 12;

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
      const me = await api.me();
      setUser(me);
      setStatus("authed");
      emit();
      if (me.must_enroll_second_factor && !quiet) toast(ENROLL_NOTICE, "info", 8000);
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

const ENROLL_NOTICE = "Admin actions are off until you add a second factor in Settings → Account.";

function finishLogin(r: LoginResponse): Me {
  if (r.password_weak) toast("Your password is weaker than Reel now requires. Change it in Settings → Account.", "info", 8000);
  return accept(r);
}

/** Where to land after signing in: an admin without a second factor goes to set one up. */
export function afterLogin(me: Me, next: string): string {
  return me.must_enroll_second_factor ? "/settings/account" : next;
}

/** Password step. Returns a challenge when the account has a second factor. */
export async function login(username: string, password: string): Promise<{ me: Me } | { challenge: MfaChallenge }> {
  const r = await api.login(withDevice({ username, password }));
  return "mfa_token" in r ? { challenge: r } : { me: finishLogin(r) };
}

/** A provider sign-in finished (#149). */
export function finishExternalLogin(r: LoginResponse): Me {
  return accept(r);
}

/** A TV's poll came back approved (#147). */
export function finishDeviceLogin(r: LoginResponse): Me {
  return accept(r);
}

export async function completeMfa(b: MfaLogin): Promise<Me> {
  return finishLogin(await api.loginMfa(b));
}

export async function completeMfaWithPasskey(mfaToken: string): Promise<Me> {
  const options = await api.loginMfaPasskey(mfaToken);
  return completeMfa({ mfa_token: mfaToken, passkey: await usePasskey(options) });
}

export async function createAdmin(username: string, password: string): Promise<Me> {
  return accept(await api.setup(withDevice({ username, password })));
}

export async function acceptInvite(code: string, username: string, password: string, link?: { ticket: string; binding: string }): Promise<Me> {
  const extra = link ? { oidc_ticket: link.ticket, oidc_binding: link.binding } : {};
  return accept(await api.acceptInvite(code, withDevice({ username, password, ...extra })));
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

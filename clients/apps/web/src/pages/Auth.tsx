import { createResource, createSignal, For, Match, onMount, Show, Switch, type JSX, type ParentProps } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { focusFirst, getServerUrl, Icon, isFileOrigin, setServerUrl } from "@reel/ui";
import { api, ApiError, type Me, type MfaChallenge } from "../api";
import { acceptInvite, afterLogin, authStatus, bootstrap, completeMfa, completeMfaWithPasskey, createAdmin, login, MIN_PASSWORD } from "../lib/auth";
import { passkeyCancelled, passkeysSupported, usePasskey } from "../lib/webauthn";
import { Field, PasswordField, useCountdown } from "../components/Form";
import { href, TV } from "../lib/mode";
import { deviceFields } from "../lib/device";
import { PairingPanel } from "./DeviceLink";
import "../styles/account.css";
import { clearInviteLink, inviteLink, oidcSupported, takePendingChallenge } from "../lib/oidc";
import { OidcButtons } from "../components/OidcButtons";

/** One centred glass panel, with the logo at the page's top left. */
function AuthLayout(props: ParentProps<{ eyebrow: string; title: string; lede: string; aside?: JSX.Element }>): JSX.Element {
  let el!: HTMLDivElement;
  onMount(() => requestAnimationFrame(() => focusFirst(el)));
  return (
    <div class="auth" ref={el}>
      <span class="logo logo--auth">
        <span class="logo__mark" aria-hidden="true"><i /></span>
        <span class="logo__word">reel</span>
      </span>
      <section class="auth__panel glass">
        <p class="eyebrow">{props.eyebrow}</p>
        <h1 class="auth__title">{props.title}</h1>
        <p class="auth__lede">{props.lede}</p>
        {props.aside}
        {props.children}
      </section>
    </div>
  );
}

function lockoutText(left: number): string {
  return left > 60 ? `Too many attempts. Try again in ${Math.ceil(left / 60)} min.` : `Too many attempts. Try again in ${left}s.`;
}

/** Post-login redirect target: same-origin paths only (`//host` and `/\host` would leave the site). */
export function safeNext(next: string | undefined): string {
  return next && /^\/(?![/\\])/.test(next) ? next : "/";
}

export function Login(): JSX.Element {
  const navigate = useNavigate();
  const [params] = useSearchParams<{ next?: string }>();
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [left, startCountdown] = useCountdown();
  // a provider sign-in on an account with a second factor continues here (#149)
  const returned = takePendingChallenge();
  const [challenge, setChallenge] = createSignal<MfaChallenge | undefined>(returned);
  const [code, setCode] = createSignal("");
  const [useRecovery, setUseRecovery] = createSignal(!!returned && !returned.methods.includes("totp") && !returned.methods.includes("passkey"));
  const done = (me: Me) => navigate(afterLogin(me, safeNext(params.next)), { replace: true });
  // With a provider configured the screen is QR + email; a password is only a way back in (#287).
  const [options] = createResource(() => api.loginOptions().catch(() => null));
  const [view, setView] = createSignal<"hosted" | "password" | "passkey">("hosted");
  const hosted = () => !!options()?.providers.length;
  const showQr = TV || (typeof matchMedia === "function" ? matchMedia("(min-width: 641px)").matches : true);
  const has = (m: string) => !!challenge()?.methods.includes(m);

  const failSecondStep = (err: unknown) => {
    if (passkeyCancelled(err)) return;
    if (err instanceof ApiError && err.status === 429) {
      startCountdown(err.retryAfter ?? 60);
      setError(undefined);
    } else if (err instanceof ApiError && err.status === 401 && /expired/.test(err.message)) {
      setChallenge(undefined);
      setPassword("");
      setError("That sign-in expired. Enter your password again.");
    } else setError(err instanceof Error ? err.message : "Couldn't sign in.");
  };

  const submitCode = async (e: Event) => {
    e.preventDefault();
    const c = challenge();
    if (!c || busy() || left() > 0) return;
    if (!code().trim()) return setError(useRecovery() ? "Enter a recovery code." : "Enter the 6-digit code.");
    setBusy(true);
    setError(undefined);
    try {
      done(await completeMfa(useRecovery() ? { mfa_token: c.mfa_token, recovery_code: code().trim() } : { mfa_token: c.mfa_token, code: code().trim() }));
    } catch (err) {
      setCode("");
      failSecondStep(err);
    } finally {
      setBusy(false);
    }
  };

  const signInWithPasskey = async () => {
    const c = challenge();
    if (!c || busy() || left() > 0) return;
    setBusy(true);
    setError(undefined);
    try {
      done(await completeMfaWithPasskey(c.mfa_token));
    } catch (err) {
      failSecondStep(err);
    } finally {
      setBusy(false);
    }
  };

  const submitEmergency = async (e: Event) => {
    e.preventDefault();
    if (busy() || left() > 0) return;
    if (!username().trim()) return setError("Enter your username.");
    setBusy(true);
    setError(undefined);
    try {
      const r = await api.passkeyLogin({ username: username().trim(), ...deviceFields() });
      done(await completeMfa({ mfa_token: r.mfa_token, passkey: await usePasskey(r.options) }));
    } catch (err) {
      failSecondStep(err);
    } finally {
      setBusy(false);
    }
  };

  const switchView = (v: "hosted" | "password" | "passkey") => {
    setError(undefined);
    setView(v);
  };

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy() || left() > 0) return;
    if (!username().trim() || !password()) return setError("Enter your username and password.");
    setBusy(true);
    setError(undefined);
    try {
      const r = await login(username().trim(), password());
      if ("challenge" in r) {
        setUseRecovery(!r.challenge.methods.includes("totp") && !r.challenge.methods.includes("passkey"));
        setChallenge(r.challenge);
      } else done(r.me);
    } catch (err) {
      if (err instanceof ApiError && err.status === 429) {
        startCountdown(err.retryAfter ?? 60);
        setError(undefined);
      } else if (err instanceof ApiError && err.status === 401) setError("That username and password don't match.");
      else setError(err instanceof Error ? err.message : "Couldn't sign in.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show
      when={!challenge()}
      fallback={
        <AuthLayout eyebrow="One more step" title="Confirm it's you" lede="Your account has a second sign-in step.">
          <form class="auth__form" onSubmit={submitCode} novalidate>
            <Show when={has("passkey") && passkeysSupported() && !useRecovery()}>
              <button type="button" class="btn btn--accent btn--lg btn--block" data-autofocus aria-disabled={busy() || left() > 0 ? "true" : undefined} onClick={signInWithPasskey}>
                <Icon name="key" /> <span>Use a passkey</span>
              </button>
            </Show>
            <Show when={has("totp") || useRecovery()}>
              <Field
                label={useRecovery() ? "Recovery code" : "6-digit code"}
                value={code()}
                onInput={(e) => setCode(e.currentTarget.value)}
                inputmode={useRecovery() ? "text" : "numeric"}
                autocomplete="one-time-code"
                autocapitalize="off"
                spellcheck={false}
                data-autofocus={!has("passkey") || !passkeysSupported() || useRecovery() ? "" : undefined}
              />
            </Show>
            <Show when={left() > 0 || error()}>
              <p class="auth__error" role="alert"><Icon name="alert" /> {left() > 0 ? lockoutText(left()) : error()}</p>
            </Show>
            <Show when={has("totp") || useRecovery()}>
              <button type="submit" class="btn btn--lg btn--block" classList={{ "btn--accent": !has("passkey") || useRecovery() }} aria-disabled={busy() || left() > 0 ? "true" : undefined}>
                <span>{busy() ? "Checking…" : "Continue"}</span>
              </button>
            </Show>
            <div class="auth__methods">
              <Show when={has("recovery") && (has("totp") || has("passkey"))}>
                <button type="button" class="btn btn--quiet btn--sm" onClick={() => { setUseRecovery((v) => !v); setCode(""); setError(undefined); }}>
                  <span>{useRecovery() ? "Use your app or passkey" : "Use a recovery code"}</span>
                </button>
              </Show>
              <button type="button" class="btn btn--quiet btn--sm" onClick={() => { setChallenge(undefined); setPassword(""); setCode(""); setError(undefined); }}>
                <span>Start over</span>
              </button>
            </div>
          </form>
        </AuthLayout>
      }
    >
    <Show when={!options.loading} fallback={<div class="splash" aria-busy="true" />}>
    <Switch>
    <Match when={hosted() && view() === "hosted"}>
      <AuthLayout
        eyebrow="Welcome back"
        title={showQr ? "Scan to sign in" : "Sign in"}
        lede={!showQr ? "Your films, your shows, your server." : TV ? "Use the camera on your phone. Nothing to type with the remote." : "Point your phone's camera at the code. No password to remember."}
      >
        <div class="auth__form auth__form--center">
          <Show when={showQr}><PairingPanel onSignedIn={done} /></Show>
          <Show when={oidcSupported()}>
            <Show when={showQr}><div class="auth__or">or</div></Show>
            <OidcButtons purpose="login" primary next={safeNext(params.next)} onError={setError} />
            <p class="auth__foot">We'll email you a 6-digit code, then ask for your authenticator app.</p>
            <Show when={!showQr}>
              <div class="auth__or">or</div>
              <p class="state__body">On a TV or computer? Scan the code it shows with this phone.</p>
            </Show>
          </Show>
          <Show when={error()}><p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p></Show>
          <div class="auth__methods">
            <Show when={options()?.password_login}>
              <button type="button" class="btn btn--quiet btn--sm" onClick={() => switchView("password")}><span>Use a password</span></button>
            </Show>
            <Show when={options()?.passkey_login && passkeysSupported()}>
              <button type="button" class="btn btn--quiet btn--sm" onClick={() => switchView("passkey")}><span>Emergency passkey</span></button>
            </Show>
            <Show when={isFileOrigin()}>
              <a class="btn btn--quiet btn--sm" href={href("/connect")}><Icon name="server" /> <span>Server: {getServerUrl().replace(/^https?:\/\//, "")} · change</span></a>
            </Show>
          </div>
        </div>
      </AuthLayout>
    </Match>
    <Match when={hosted() && view() === "passkey"}>
      <AuthLayout eyebrow="Emergency sign-in" title="Use your passkey" lede="For when the sign-in provider is unavailable. Works from your home network or Tailscale.">
        <form class="auth__form" onSubmit={submitEmergency} novalidate>
          <Field label="Username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username webauthn" autocapitalize="off" spellcheck={false} data-autofocus />
          <Show when={left() > 0 || error()}>
            <p class="auth__error" role="alert"><Icon name="alert" /> {left() > 0 ? lockoutText(left()) : error()}</p>
          </Show>
          <button type="submit" class="btn btn--primary btn--lg btn--block" aria-disabled={busy() || left() > 0 ? "true" : undefined}>
            <Icon name="key" /> <span>{busy() ? "Waiting for your passkey…" : "Use a passkey"}</span>
          </button>
          <button type="button" class="btn btn--quiet btn--sm" onClick={() => switchView("hosted")}><Icon name="back" /> <span>Back</span></button>
        </form>
      </AuthLayout>
    </Match>
    <Match when={true}>
    <AuthLayout eyebrow="Welcome back" title="Sign in" lede="Your films, your shows, your server.">
      <form class="auth__form" onSubmit={submit} novalidate>
        <Field label="Username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username" autocapitalize="off" spellcheck={false} data-autofocus />
        <PasswordField label="Password" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="current-password" />
        <Show when={left() > 0 || error()}>
          <p class="auth__error" role="alert">
            <Icon name="alert" /> {left() > 0 ? lockoutText(left()) : error()}
          </p>
        </Show>
        <button type="submit" class="btn btn--accent btn--lg btn--block" aria-disabled={busy() || left() > 0 ? "true" : undefined}>
          <span>{busy() ? "Signing in…" : "Sign in"}</span>
        </button>
        <Show
          when={hosted()}
          fallback={
            <>
              <a class="btn btn--block" href={href("/login/device")}>
                <Icon name="tv" /> <span>Sign in with your phone</span>
              </a>
              <p class="auth__foot">Got an invite link? Open it on this device to create your account.</p>
            </>
          }
        >
          <button type="button" class="btn btn--quiet btn--sm" onClick={() => switchView("hosted")}><Icon name="back" /> <span>Back</span></button>
        </Show>
        <Show when={isFileOrigin()}>
          <a class="btn btn--quiet btn--sm" href={href("/connect")}>
            <Icon name="server" /> <span>Server: {getServerUrl().replace(/^https?:\/\//, "")} · change</span>
          </a>
        </Show>
      </form>
    </AuthLayout>
    </Match>
    </Switch>
    </Show>
    </Show>
  );
}

export function Setup(): JSX.Element {
  const navigate = useNavigate();
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const tooShort = () => password().length > 0 && password().length < MIN_PASSWORD;
  const mismatch = () => confirm().length > 0 && confirm() !== password();

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!username().trim()) return setError("Pick a username.");
    if (password().length < MIN_PASSWORD) return setError(`The password needs at least ${MIN_PASSWORD} characters.`);
    if (password() !== confirm()) return setError("The passwords don't match.");
    setBusy(true);
    setError(undefined);
    try {
      const me = await createAdmin(username().trim(), password());
      navigate(afterLogin(me, "/admin/libraries"), { replace: true });
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setError("This server is already set up. Sign in instead.");
        void bootstrap();
      } else setError(err instanceof Error ? err.message : "Couldn't create the account.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      eyebrow="First run"
      title="Set up your server"
      lede="Create the admin account. You'll add libraries and invite people next — nobody else can sign in until you do."
      aside={
        <ol class="auth__steps">
          <li class="is-current">Admin account</li>
          <li>Add a library</li>
          <li>Invite friends</li>
        </ol>
      }
    >
      <form class="auth__form" onSubmit={submit} novalidate>
        <Field label="Admin username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username" autocapitalize="off" spellcheck={false} data-autofocus />
        <PasswordField label="Password" hint={`At least ${MIN_PASSWORD} characters`} error={tooShort() && `At least ${MIN_PASSWORD} characters`} value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="new-password" />
        <PasswordField label="Confirm password" error={mismatch() && "Doesn't match"} value={confirm()} onInput={(e) => setConfirm(e.currentTarget.value)} autocomplete="new-password" />
        <Show when={error()}>
          <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
        </Show>
        <button type="submit" class="btn btn--accent btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
          <span>{busy() ? "Creating…" : "Create admin account"}</span>
        </button>
      </form>
    </AuthLayout>
  );
}

export function Invite(): JSX.Element {
  const navigate = useNavigate();
  const params = useParams<{ code: string }>();
  const [info] = createResource(() => params.code, (c) => api.inviteInfo(c));
  // with a provider configured an account has no password: it signs up there first (#287)
  const [inviteOptions] = createResource(() => api.loginOptions().catch(() => null));
  const needsProvider = () => !!inviteOptions()?.providers.length;
  const linked = () => inviteLink(params.code);
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!username().trim()) return setError("Pick a username.");
    if (!linked() && password().length < MIN_PASSWORD) return setError(`The password needs at least ${MIN_PASSWORD} characters.`);
    setBusy(true);
    setError(undefined);
    try {
      await acceptInvite(params.code, username().trim(), linked() ? undefined : password(), linked());
      clearInviteLink();
      navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) setError("That username is taken — try another.");
      else if (err instanceof ApiError && (err.status === 404 || err.status === 410)) setError("This invite has expired or was already used.");
      else setError(err instanceof Error ? err.message : "Couldn't accept the invite.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Switch>
      <Match when={info.loading}>
        <div class="splash" aria-busy="true" />
      </Match>
      <Match when={info.error || (info() && !info()!.valid)}>
        <AuthLayout eyebrow="Invite" title="This invite isn't valid" lede="It may have expired or already been used. Ask whoever sent it for a new link.">
          <a class="btn btn--lg" href={href(authStatus() === "authed" ? "/" : "/login")} data-autofocus>
            <Icon name="back" /> <span>{authStatus() === "authed" ? "Go home" : "Sign in instead"}</span>
          </a>
        </AuthLayout>
      </Match>
      <Match when={info()}>
        {(i) => (
          <AuthLayout
            eyebrow="You're invited"
            title="Join this Reel server"
            lede="Create an account to start watching."
            aside={
              <Show when={i().library_names.length}>
                <div class="auth__libs">
                  <p class="eyebrow">You'll have access to</p>
                  <ul>
                    <For each={i().library_names}>{(n) => <li><Icon name="film" /> {n}</li>}</For>
                  </ul>
                  <Show when={i().expires_at}>
                    <p class="auth__fine">Invite expires {new Date(i().expires_at! * 1000).toLocaleDateString()}</p>
                  </Show>
                </div>
              </Show>
            }
          >
            <form class="auth__form" onSubmit={submit} novalidate>
              <Show
                when={inviteLink(params.code)}
                fallback={<OidcButtons purpose="invite" invite={params.code} primary={needsProvider()} onError={setError} />}
              >
                {(l) => <p class="state__body" role="status"><Icon name="check" /> Signed in with {l().provider}. Pick a username to finish.</p>}
              </Show>
              <Show when={linked() || !needsProvider()}>
                <Field label="Choose a username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username" autocapitalize="off" spellcheck={false} data-autofocus />
              </Show>
              <Show when={!linked() && !needsProvider()}>
                <PasswordField label="Choose a password" hint={`At least ${MIN_PASSWORD} characters`} value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="new-password" />
              </Show>
              <Show when={error()}>
                <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
              </Show>
              <Show when={linked() || !needsProvider()}>
                <button type="submit" class="btn btn--accent btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
                  <span>{busy() ? "Creating account…" : "Create account"}</span>
                </button>
              </Show>
            </form>
          </AuthLayout>
        )}
      </Match>
    </Switch>
  );
}

/** Packaged TV app: ask for the server address once. */
export function Connect(): JSX.Element {
  const navigate = useNavigate();
  const [addr, setAddr] = createSignal(getServerUrl().replace(/^https?:\/\//, ""));
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    const url = setServerUrl(addr());
    if (!url) return setError("That doesn't look like an address. Try 192.168.1.20:8420");
    setBusy(true);
    setError(undefined);
    try {
      const r = await fetch(`${url}/api/v1/health`);
      if (!r.ok) throw new Error();
      await bootstrap();
      // "/" lets the auth guard pick setup, login or home for this server.
      navigate("/", { replace: true });
    } catch {
      setError(`No Reel server answered at ${url}. Check the address and that the server is running.`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <AuthLayout eyebrow="First run" title="Server address" lede="Enter the address of the computer running Reel on your network — it's shown in the server's log when it starts.">
      <form class="auth__form" onSubmit={submit} novalidate>
        <Field label="Server address" placeholder="192.168.1.20:8420" value={addr()} onInput={(e) => setAddr(e.currentTarget.value)} autocapitalize="off" spellcheck={false} inputmode="url" data-autofocus />
        <Show when={error()}>
          <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
        </Show>
        <button type="submit" class="btn btn--accent btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
          <span>{busy() ? "Connecting…" : "Connect"}</span>
        </button>
      </form>
    </AuthLayout>
  );
}

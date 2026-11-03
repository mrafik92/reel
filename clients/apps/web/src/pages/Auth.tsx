import { createResource, createSignal, For, Match, onMount, Show, Switch, type JSX, type ParentProps } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { focusFirst, getServerUrl, Icon, isFileOrigin, setServerUrl } from "@reel/ui";
import { api, ApiError } from "../api";
import { acceptInvite, authStatus, bootstrap, createAdmin, login } from "../lib/auth";
import { Field, PasswordField, useCountdown } from "../components/Form";
import { href } from "../lib/mode";

/** Two-column auth layout: wordmark + message left, form right. Stacks on narrow screens. */
function AuthLayout(props: ParentProps<{ eyebrow: string; title: string; lede: string; aside?: JSX.Element }>): JSX.Element {
  let el!: HTMLDivElement;
  onMount(() => requestAnimationFrame(() => focusFirst(el)));
  return (
    <div class="auth" ref={el}>
      <div class="auth__glow" aria-hidden="true" />
      <section class="auth__intro">
        <span class="logo logo--auth">
          <span class="logo__mark" aria-hidden="true"><i /></span>
          <span class="logo__word">reel</span>
        </span>
        <p class="eyebrow eyebrow--accent">{props.eyebrow}</p>
        <h1 class="auth__title">{props.title}</h1>
        <p class="auth__lede">{props.lede}</p>
        {props.aside}
      </section>
      <section class="auth__panel">{props.children}</section>
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

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy() || left() > 0) return;
    if (!username().trim() || !password()) return setError("Enter your username and password.");
    setBusy(true);
    setError(undefined);
    try {
      await login(username().trim(), password());
      navigate(safeNext(params.next), { replace: true });
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
    <AuthLayout eyebrow="Welcome back" title="Sign in" lede="Your films, your shows, your server.">
      <form class="auth__form" onSubmit={submit} novalidate>
        <Field label="Username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username" autocapitalize="off" spellcheck={false} data-autofocus />
        <PasswordField label="Password" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="current-password" />
        <Show when={left() > 0 || error()}>
          <p class="auth__error" role="alert">
            <Icon name="alert" /> {left() > 0 ? lockoutText(left()) : error()}
          </p>
        </Show>
        <button type="submit" class="btn btn--primary btn--lg btn--block" aria-disabled={busy() || left() > 0 ? "true" : undefined}>
          <span>{busy() ? "Signing in…" : "Sign in"}</span>
        </button>
        <p class="auth__foot">Got an invite link? Open it on this device to create your account.</p>
        <Show when={isFileOrigin()}>
          <a class="btn btn--quiet btn--sm" href={href("/connect")}>
            <Icon name="server" /> <span>Server: {getServerUrl().replace(/^https?:\/\//, "")} · change</span>
          </a>
        </Show>
      </form>
    </AuthLayout>
  );
}

export function Setup(): JSX.Element {
  const navigate = useNavigate();
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const tooShort = () => password().length > 0 && password().length < 8;
  const mismatch = () => confirm().length > 0 && confirm() !== password();

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!username().trim()) return setError("Pick a username.");
    if (password().length < 8) return setError("The password needs at least 8 characters.");
    if (password() !== confirm()) return setError("The passwords don't match.");
    setBusy(true);
    setError(undefined);
    try {
      await createAdmin(username().trim(), password());
      navigate("/admin/libraries", { replace: true });
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
        <PasswordField label="Password" hint="At least 8 characters" error={tooShort() && "At least 8 characters"} value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="new-password" />
        <PasswordField label="Confirm password" error={mismatch() && "Doesn't match"} value={confirm()} onInput={(e) => setConfirm(e.currentTarget.value)} autocomplete="new-password" />
        <Show when={error()}>
          <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
        </Show>
        <button type="submit" class="btn btn--primary btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
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
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!username().trim()) return setError("Pick a username.");
    if (password().length < 8) return setError("The password needs at least 8 characters.");
    setBusy(true);
    setError(undefined);
    try {
      await acceptInvite(params.code, username().trim(), password());
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
              <Field label="Choose a username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocomplete="username" autocapitalize="off" spellcheck={false} data-autofocus />
              <PasswordField label="Choose a password" hint="At least 8 characters" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="new-password" />
              <Show when={error()}>
                <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
              </Show>
              <button type="submit" class="btn btn--primary btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
                <span>{busy() ? "Creating account…" : "Create account"}</span>
              </button>
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
        <button type="submit" class="btn btn--primary btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}>
          <span>{busy() ? "Connecting…" : "Connect"}</span>
        </button>
      </form>
    </AuthLayout>
  );
}

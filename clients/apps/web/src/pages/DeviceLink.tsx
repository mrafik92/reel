import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useNavigate, useSearchParams } from "@solidjs/router";
import { focusFirst, Icon } from "@reel/ui";
import { api, ApiError, type DeviceLinkInfo, type DevicePairing, type LoginResponse } from "../api";
import { Field } from "../components/Form";
import { QrCode } from "../components/QrCode";
import { afterLogin, finishDeviceLogin } from "../lib/auth";
import { deviceFields } from "../lib/device";
import { href } from "../lib/mode";
import "../styles/account.css";

function Layout(props: { eyebrow: string; title: string; lede: string; children: JSX.Element }): JSX.Element {
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
        {props.children}
      </section>
    </div>
  );
}

/** On the TV: show a code and a QR code, and wait for a signed-in phone to approve it (#147). */
export function DeviceLogin(): JSX.Element {
  const navigate = useNavigate();
  const [pairing, setPairing] = createSignal<DevicePairing>();
  const [error, setError] = createSignal<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  onCleanup(() => {
    stopped = true;
    if (timer) clearTimeout(timer);
  });
  const later = (fn: () => void, seconds: number) => {
    if (!stopped) timer = setTimeout(fn, Math.max(1, seconds) * 1000);
  };

  const start = async () => {
    if (stopped) return;
    try {
      const p = await api.deviceStart(deviceFields());
      setPairing(p);
      setError(undefined);
      later(() => void poll(p), p.interval_s);
    } catch (e) {
      setPairing(undefined);
      const wait = e instanceof ApiError && e.retryAfter ? e.retryAfter : 15;
      setError(e instanceof ApiError && e.status === 429 ? "Too many codes requested. Trying again shortly." : "Can't reach the server. Trying again.");
      later(() => void start(), wait);
    }
  };

  const poll = async (p: DevicePairing) => {
    if (stopped) return;
    try {
      const r = await api.devicePoll(p.device_code);
      if (stopped) return;
      if (r && "token" in r) {
        navigate(afterLogin(finishDeviceLogin(r as LoginResponse), "/"), { replace: true });
        return;
      }
      later(() => void poll(p), p.interval_s);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return void start(); // expired or refused: new code
      later(() => void poll(p), e instanceof ApiError && e.retryAfter ? e.retryAfter : p.interval_s * 2);
    }
  };

  onMount(() => void start());

  return (
    <Layout eyebrow="Sign in" title="Use your phone" lede="Scan the code with a phone that's signed in to Reel, or open the address and enter the code.">
      <div class="auth__form">
        <Show when={pairing()} fallback={<p class="state__body" role="status">{error() ?? "Getting a code…"}</p>}>
          {(p) => (
            <div class="devicelink">
              <QrCode value={p().verification_uri_complete} label="QR code to approve this TV" size={232} />
              <div>
                <p class="state__body">{p().verification_uri.replace(/^https?:\/\//, "")}</p>
                <p class="devicelink__code">{p().user_code}</p>
                <p class="state__body" role="status">Waiting for approval…</p>
              </div>
            </div>
          )}
        </Show>
        <a class="btn btn--quiet btn--sm" href={href("/login")} data-autofocus>
          <Icon name="back" /> <span>Use a password instead</span>
        </a>
      </div>
    </Layout>
  );
}

/** On the phone: check what's asking to sign in, then approve it. */
export function DeviceLink(): JSX.Element {
  const navigate = useNavigate();
  const [params] = useSearchParams<{ code?: string }>();
  const [code, setCode] = createSignal(params.code ?? "");
  const [info, setInfo] = createSignal<DeviceLinkInfo>();
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [result, setResult] = createSignal<"approved" | "denied">();

  const act = async (fn: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true);
    setError(undefined);
    try {
      await fn();
    } catch (e) {
      setInfo(undefined);
      setError(e instanceof Error ? e.message : "That didn't work.");
    } finally {
      setBusy(false);
    }
  };
  const lookup = (e?: Event) => {
    e?.preventDefault();
    if (!code().trim()) return setError("Enter the code shown on the TV.");
    void act(async () => void setInfo(await api.linkLookup(code().trim())));
  };
  // A QR link only fills the code in. Approving always takes a tap here.
  onMount(() => {
    if (params.code) lookup();
  });

  return (
    <Layout eyebrow="Link a device" title="Sign in a TV" lede="Only approve a code you can see on your own screen. The device gets access to your account.">
      <div class="auth__form">
        <Show when={result()}>
          <p class="state__body" role="status">
            <Icon name="check" /> {result() === "approved" ? "Approved. The TV will sign in within a few seconds." : "Refused. That code no longer works."}
          </p>
          <button type="button" class="btn btn--accent btn--lg btn--block" data-autofocus onClick={() => navigate("/", { replace: true })}><span>Done</span></button>
        </Show>
        <Show when={!result() && info()}>
          {(i) => (
            <>
              <dl class="devicelink__facts">
                <dt>Device</dt><dd>{i().device_name || "Unnamed device"}</dd>
                <Show when={i().client}><dt>App</dt><dd>{i().client}</dd></Show>
                <Show when={i().ip}><dt>Address</dt><dd>{i().ip}</dd></Show>
                <dt>Code</dt><dd>{code().toUpperCase()}</dd>
              </dl>
              <Show when={error()}><p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p></Show>
              <button type="button" class="btn btn--accent btn--lg btn--block" data-autofocus aria-disabled={busy() ? "true" : undefined} onClick={() => void act(async () => { await api.linkApprove(code().trim()); setResult("approved"); })}>
                <span>Approve this device</span>
              </button>
              <button type="button" class="btn btn--lg btn--block" aria-disabled={busy() ? "true" : undefined} onClick={() => void act(async () => { await api.linkDeny(code().trim()); setResult("denied"); })}>
                <span>It's not mine</span>
              </button>
            </>
          )}
        </Show>
        <Show when={!result() && !info()}>
          <form onSubmit={lookup} novalidate>
            <Field label="Code on the TV" placeholder="BCDF-GHJK" value={code()} onInput={(e) => setCode(e.currentTarget.value)} autocapitalize="characters" autocomplete="off" spellcheck={false} maxLength={12} data-autofocus />
            <Show when={error()}><p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p></Show>
            <button type="submit" class="btn btn--accent btn--lg btn--block" aria-disabled={busy() ? "true" : undefined}><span>{busy() ? "Checking…" : "Continue"}</span></button>
          </form>
        </Show>
      </div>
    </Layout>
  );
}

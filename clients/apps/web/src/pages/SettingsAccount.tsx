import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, ApiError, type TotpEnrollment } from "../api";
import { Panel, ConfirmButton } from "../components/AdminKit";
import { Field, PasswordField } from "../components/Form";
import { QrCode } from "../components/QrCode";
import { bootstrap, currentUser, MIN_PASSWORD } from "../lib/auth";
import { oidcSupported, startOidcLink } from "../lib/oidc";
import { toast } from "../lib/toast";
import { createPasskey, passkeyCancelled, passkeysSupported } from "../lib/webauthn";
import "../styles/account.css";

const message = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

function when(ts?: number | null): string {
  return ts ? new Date(ts * 1000).toLocaleDateString() : "never";
}

/** Shown once: the only moment anyone can read these codes. */
function RecoveryCodes(props: { codes: string[]; onDone: () => void }): JSX.Element {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.codes.join("\n"));
      toast("Recovery codes copied", "success", 2000);
    } catch {
      toast("Couldn't copy. Write them down instead.", "error");
    }
  };
  return (
    <div class="account__codes" role="status">
      <p><strong>Save these recovery codes now.</strong> Each works once if you lose your device, and Reel can't show them again.</p>
      <ul class="account__codelist">
        <For each={props.codes}>{(c) => <li><code>{c}</code></li>}</For>
      </ul>
      <div class="account__row">
        <button type="button" class="btn btn--sm" onClick={copy}><Icon name="copy" /> <span>Copy</span></button>
        <button type="button" class="btn btn--primary btn--sm" data-autofocus onClick={props.onDone}><span>I've saved them</span></button>
      </div>
    </div>
  );
}

/** Settings → Account: password, second factor and signed-in devices (#146, #148). */
export default function SettingsAccount(): JSX.Element {
  const [sec, { refetch }] = createResource(() => api.security());
  const [devices, devicesCtl] = createResource(() => api.mySessions());
  const [identities, identitiesCtl] = createResource(() => api.myIdentities());
  const [providers] = createResource(() => api.oidcProviders().catch(() => []));
  const [password, setPassword] = createSignal("");
  const [enrolling, setEnrolling] = createSignal<TotpEnrollment>();
  const [code, setCode] = createSignal("");
  const [keyName, setKeyName] = createSignal("");
  const [codes, setCodes] = createSignal<string[]>();
  const [busy, setBusy] = createSignal(false);
  const [newPassword, setNewPassword] = createSignal("");
  // #287: no stored password means account changes are confirmed with the authenticator app
  const passwordless = () => currentUser()?.has_password === false;
  const manageFactors = () => !passwordless() || !!sec()?.totp;
  const passwordWaitsForPasskey = () => !passwordless() && currentUser()?.role === "admin" && !!identities()?.length && !sec()?.passkeys.length;

  /** Run one change; refresh the account afterwards so admin powers update straight away. */
  const run = async (fn: () => Promise<void>, fallback: string) => {
    if (busy()) return;
    setBusy(true);
    try {
      await fn();
      await refetch();
      await bootstrap(true);
    } catch (e) {
      if (!passkeyCancelled(e)) toast(message(e, fallback), "error", 6000);
    } finally {
      setBusy(false);
    }
  };

  const needPassword = () => {
    if (password()) return true;
    toast(passwordless() ? "Enter a code from your authenticator app first." : "Enter your current password first.", "error");
    return false;
  };

  const startTotp = () => needPassword() && run(async () => {
    setEnrolling(await api.totpStart(password()));
    setCode("");
  }, "Couldn't start the setup.");

  const confirmTotp = (e: Event) => {
    e.preventDefault();
    void run(async () => {
      const r = await api.totpConfirm(code());
      setEnrolling(undefined);
      setPassword("");
      if (r.recovery_codes) setCodes(r.recovery_codes);
      toast("Authenticator app added", "success");
    }, "That code didn't work.");
  };

  const addPasskey = () => {
    if (!needPassword()) return;
    if (!keyName().trim()) return void toast("Give this passkey a name, like “MacBook”.", "error");
    void run(async () => {
      const options = await api.passkeyStart(password(), keyName().trim());
      const r = await api.passkeyFinish(await createPasskey(options));
      setKeyName("");
      setPassword("");
      if (r.recovery_codes) setCodes(r.recovery_codes);
      toast("Passkey added", "success");
    }, "Couldn't add the passkey.");
  };

  const changePassword = (e: Event) => {
    e.preventDefault();
    if (!needPassword()) return;
    if (newPassword().length < MIN_PASSWORD) return void toast(`The new password needs at least ${MIN_PASSWORD} characters.`, "error");
    void run(async () => {
      await api.changePassword(password(), newPassword());
      setPassword("");
      setNewPassword("");
      void devicesCtl.refetch();
      toast("Password changed. Other devices were signed out.", "success");
    }, "Couldn't change the password.");
  };

  return (
    <div class="account">
      <Show when={sec()?.must_enroll}>
        <p class="account__notice" role="alert">
          <Icon name="alert" /> <span>Admin actions stay off until this account has a passkey or an authenticator app. Add one below, from your home network or Tailscale.</span>
        </p>
      </Show>
      <Show when={currentUser()?.role === "admin" && currentUser()?.admin_powers === false && !sec()?.must_enroll}>
        <p class="account__notice"><Icon name="alert" /> <span>You're on a public address, so admin actions are off. Everything else works as usual.</span></p>
      </Show>

      <Show when={passwordWaitsForPasskey()}>
        <p class="account__notice"><Icon name="key" /> <span>Add a passkey below as your emergency sign-in. Reel then deletes your stored password, so only your provider and this passkey can sign you in.</span></p>
      </Show>
      <Show when={passwordless() && !sec()?.totp}>
        <p class="account__notice"><Icon name="check" /> <span>This account has no password. Your email code and 2FA are managed by your sign-in provider.</span></p>
      </Show>

      <Show when={codes()}>{(c) => <RecoveryCodes codes={c()} onDone={() => setCodes(undefined)} />}</Show>

      <Show when={manageFactors()}>
      <Panel title="Confirm it's you" icon="key">
        <Show
          when={passwordless()}
          fallback={<PasswordField label="Current password" hint="Needed to change your password or sign-in methods" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="current-password" />}
        >
          <Field label="Authenticator code" hint="Needed to change your sign-in methods" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} inputmode="numeric" autocomplete="one-time-code" maxLength={7} />
        </Show>
      </Panel>

      <Panel title="Authenticator app" icon="key">
        <Show
          when={sec()?.totp}
          fallback={
            <Show
              when={enrolling()}
              fallback={
                <div class="account__row">
                  <p class="state__body">Use a 6-digit code from an app like Aegis, 1Password or Google Authenticator.</p>
                  <button type="button" class="btn btn--sm" onClick={startTotp}><Icon name="plus" /> <span>Set up</span></button>
                </div>
              }
            >
              {(t) => (
                <form class="account__enroll" onSubmit={confirmTotp} novalidate>
                  <QrCode value={t().otpauth_uri} label="QR code for your authenticator app" />
                  <div>
                    <p class="state__body">Scan this in your app, or enter the key by hand:</p>
                    <p><code class="account__secret">{t().secret}</code></p>
                    <Field label="6-digit code" value={code()} onInput={(e) => setCode(e.currentTarget.value)} inputmode="numeric" autocomplete="one-time-code" maxLength={7} data-autofocus />
                    <div class="account__row">
                      <button type="submit" class="btn btn--primary btn--sm"><span>Turn on</span></button>
                      <button type="button" class="btn btn--sm" onClick={() => setEnrolling(undefined)}><span>Cancel</span></button>
                    </div>
                  </div>
                </form>
              )}
            </Show>
          }
        >
          <div class="account__row">
            <p class="state__body"><Icon name="check" /> On</p>
            <ConfirmButton label="Remove" onConfirm={() => { if (needPassword()) return run(async () => { await api.totpRemove(password()); setPassword(""); toast("Authenticator app removed", "success"); }, "Couldn't remove it."); }} />
          </div>
        </Show>
      </Panel>

      <Panel title="Passkeys" icon="key">
        <Show when={sec()?.passkeys_available} fallback={<p class="state__body">Passkeys need the server's public address (<code>REEL_PUBLIC_URL</code>) to be set.</p>}>
          <ul class="list">
            <For each={sec()?.passkeys ?? []}>
              {(k) => (
                <li class="list__row">
                  <div class="list__main">
                    <strong>{k.name}</strong>
                    <span class="list__sub">Added {when(k.created_at)} · last used {when(k.last_used_at)}</span>
                  </div>
                  <div class="list__actions">
                    <ConfirmButton label="Remove" onConfirm={() => { if (needPassword()) return run(async () => { await api.passkeyRemove(k.id, password()); setPassword(""); toast("Passkey removed", "success"); }, "Couldn't remove it."); }} />
                  </div>
                </li>
              )}
            </For>
          </ul>
          <Show when={passkeysSupported()} fallback={<p class="state__body">This device can't create passkeys. Add one from a phone or computer.</p>}>
            <div class="account__row">
              <Field label="Name for a new passkey" placeholder="MacBook" value={keyName()} onInput={(e) => setKeyName(e.currentTarget.value)} maxLength={64} />
              <button type="button" class="btn btn--sm" onClick={addPasskey}><Icon name="plus" /> <span>Add passkey</span></button>
            </div>
          </Show>
        </Show>
      </Panel>

      </Show>

      <Show when={manageFactors() && (sec()?.totp || sec()?.passkeys.length)}>
        <Panel title="Recovery codes" icon="key">
          <div class="account__row">
            <p class="state__body">{sec()?.recovery_codes_left ?? 0} left</p>
            <ConfirmButton label="Make new codes" confirm="Replace all codes?" onConfirm={() => { if (needPassword()) return run(async () => { const r = await api.newRecoveryCodes(password()); setPassword(""); if (r.recovery_codes) setCodes(r.recovery_codes); }, "Couldn't make new codes."); }} />
          </div>
        </Panel>
      </Show>

      <Show when={providers()?.length || identities()?.length}>
        <Panel title="Linked sign-ins" icon="user">
          <ul class="list">
            <For each={identities() ?? []}>
              {(i) => (
                <li class="list__row">
                  <div class="list__main">
                    <strong>{i.provider}</strong>
                    <span class="list__sub">{i.label ? `${i.label} · ` : ""}last used {when(i.last_used_at)}</span>
                  </div>
                  <div class="list__actions">
                    <ConfirmButton label="Unlink" onConfirm={() => { if (needPassword()) return run(async () => { await api.identityRemove(i.id, password()); setPassword(""); await identitiesCtl.refetch(); toast("Unlinked", "success"); }, "Couldn't unlink it."); }} />
                  </div>
                </li>
              )}
            </For>
          </ul>
          <Show when={manageFactors() && oidcSupported()} fallback={<p class="state__body">Link a provider from a phone or computer.</p>}>
            <div class="auth__methods">
              <For each={providers() ?? []}>
                {(p) => (
                  <button type="button" class="btn btn--sm" onClick={() => { if (needPassword()) void run(() => startOidcLink(p, password()), `Couldn't reach ${p.name}.`); }}>
                    <Icon name="plus" /> <span>Link {p.name}</span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </Panel>
      </Show>

      <Show when={!passwordless()}>
      <Panel title="Password" icon="key">
        <form onSubmit={changePassword} novalidate>
          <PasswordField label="New password" hint={`At least ${MIN_PASSWORD} characters. A few unrelated words work well.`} value={newPassword()} onInput={(e) => setNewPassword(e.currentTarget.value)} autocomplete="new-password" />
          <button type="submit" class="btn btn--sm"><span>Change password</span></button>
        </form>
      </Panel>
      </Show>

      <Panel title="Signed-in devices" icon="tv">
        <ul class="list">
          <For each={devices() ?? []}>
            {(d) => (
              <li class="list__row">
                <div class="list__main">
                  <strong>{d.device_name || d.client || "Unknown device"}<Show when={d.current}> <span class="list__you">this device</span></Show></strong>
                  <span class="list__sub">{d.ip ?? "unknown address"} · last seen {new Date(d.last_seen * 1000).toLocaleString()}</span>
                </div>
                <Show when={!d.current}>
                  <div class="list__actions">
                    <ConfirmButton label="Sign out" onConfirm={async () => {
                      try {
                        await api.revokeMySession(d.id);
                      } catch (e) {
                        if (!(e instanceof ApiError && e.status === 404)) toast(message(e, "Couldn't sign that device out."), "error");
                      }
                      void devicesCtl.refetch();
                    }} />
                  </div>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Panel>
    </div>
  );
}

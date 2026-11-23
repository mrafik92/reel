import { createSignal, onMount, Show, type JSX } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { Icon } from "@reel/ui";
import { api, type LoginResponse } from "../api";
import { afterLogin, finishExternalLogin } from "../lib/auth";
import { href } from "../lib/mode";
import { saveInviteLink, setPendingChallenge, takeOidcPending } from "../lib/oidc";
import { toast } from "../lib/toast";

const REASONS: Record<string, string> = {
  denied: "The sign-in was cancelled.",
  expired: "That sign-in expired. Try again.",
  failed: "The provider's answer couldn't be verified. Try again.",
};

/** The provider sent the browser back: finish whatever this trip was for (#149). */
export default function OidcReturn(): JSX.Element {
  const navigate = useNavigate();
  const [error, setError] = createSignal<string>();

  onMount(async () => {
    const hash = new URLSearchParams(location.hash.replace(/^#/, ""));
    // the ticket shouldn't stay in the address bar or the history
    history.replaceState(history.state, "", location.pathname);
    const pending = takeOidcPending();
    const ticket = hash.get("ticket");
    if (!ticket || !pending) {
      setError(REASONS[hash.get("error") ?? ""] ?? REASONS.expired);
      return;
    }
    try {
      if (pending.purpose === "link") {
        await api.identityComplete(ticket, pending.binding);
        toast(`${pending.provider} sign-in linked`, "success");
        navigate("/settings/account", { replace: true });
      } else if (pending.purpose === "invite" && pending.invite) {
        saveInviteLink({ code: pending.invite, ticket, binding: pending.binding, provider: pending.provider });
        navigate(`/invite/${encodeURIComponent(pending.invite)}`, { replace: true });
      } else {
        const r = await api.oidcExchange(ticket, pending.binding);
        if ("mfa_token" in r) {
          setPendingChallenge(r);
          navigate("/login", { replace: true });
        } else {
          navigate(afterLogin(finishExternalLogin(r as LoginResponse), pending.next ?? "/"), { replace: true });
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : REASONS.failed);
    }
  });

  return (
    <div class="auth">
      <div class="auth__glow" aria-hidden="true" />
      <section class="auth__panel">
        <Show when={error()} fallback={<p class="state__body" role="status">Finishing sign-in…</p>}>
          <div class="auth__form">
            <p class="auth__error" role="alert"><Icon name="alert" /> {error()}</p>
            <a class="btn btn--primary btn--lg btn--block" href={href("/login")} data-autofocus><span>Back to sign in</span></a>
          </div>
        </Show>
      </section>
    </div>
  );
}

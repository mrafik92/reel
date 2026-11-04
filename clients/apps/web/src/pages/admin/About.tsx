import { createResource, Show, type JSX } from "solid-js";
import { http } from "../../api/http";

type Health = { status: string; version?: string; git_sha?: string; build_date?: string };

const WEB_SHA = (import.meta.env.VITE_GIT_SHA || "").slice(0, 7);
const WEB_DATE = import.meta.env.VITE_BUILD_DATE || "";

function short(sha?: string): string {
  return sha ? sha.slice(0, 7) : "dev";
}

function when(iso?: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** Server + web app versions. Shown to everyone on the Settings page. */
export default function About(): JSX.Element {
  const [health] = createResource(() => http.get<Health>("/health").catch(() => null));
  // A web bundle older than the server (e.g. a cached tab after a deploy) is worth a reload hint.
  const stale = () => {
    const s = health()?.git_sha;
    return !!(s && WEB_SHA && !s.startsWith(WEB_SHA));
  };
  return (
    <section class="admin__about" aria-label="About Reel">
      <p class="admin__about-title">About</p>
      <dl>
        <dt>Server</dt>
        <dd>
          <Show when={health()} fallback={<span>{health.loading ? "…" : "unreachable"}</span>}>
            {(h) => (
              <>
                v{h().version ?? "?"} · <code>{short(h().git_sha)}</code>
                <Show when={h().build_date}>
                  <small>built {when(h().build_date)}</small>
                </Show>
              </>
            )}
          </Show>
        </dd>
        <dt>Web app</dt>
        <dd>
          <code>{WEB_SHA || "dev"}</code>
          <Show when={WEB_DATE}>
            <small>built {when(WEB_DATE)}</small>
          </Show>
        </dd>
      </dl>
      <Show when={stale()}>
        <button type="button" class="btn btn--ghost admin__about-reload" onClick={() => location.reload()}>
          A newer version is on the server — reload
        </button>
      </Show>
    </section>
  );
}

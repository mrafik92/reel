import { render } from "solid-js/web";
import { createResource, Show } from "solid-js";
import { createReelClient } from "@reel/sdk";
import { detectWebProfile } from "@reel/ui";

const client = createReelClient(import.meta.env.VITE_REEL_URL ?? "");

function App() {
  const [health] = createResource(async () => {
    const { data, error } = await client.GET("/api/v1/health");
    if (error || !data) throw new Error("server unreachable");
    return data;
  });
  const profile = detectWebProfile();
  return (
    <main>
      <h1>Reel</h1>
      <Show when={!health.loading} fallback={<p>Checking server…</p>}>
        <Show when={!health.error} fallback={<p>Server: offline</p>}>
          <p>Server: {health()?.status} (v{health()?.version})</p>
        </Show>
      </Show>
      <pre>{JSON.stringify(profile)}</pre>
    </main>
  );
}

render(() => <App />, document.getElementById("root")!);

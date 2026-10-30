import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, type NotificationTarget } from "../../api";
import { toast } from "../../lib/toast";
import { Field, PasswordField } from "../../components/Form";
import { ConfirmButton, Panel } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

type Kind = "discord" | "telegram" | "webhook";
const EVENTS = [
  { id: "requested", label: "Requested" },
  { id: "approved", label: "Approved" },
  { id: "declined", label: "Declined" },
  { id: "available", label: "Available" },
  { id: "failed", label: "Failed" },
  { id: "issue", label: "Issue reported" },
];
const KIND_LABEL: Record<string, string> = { discord: "Discord", telegram: "Telegram", webhook: "Webhook" };

export default function Notifications(): JSX.Element {
  const [list, { refetch }] = createResource(() => api.notifications());
  const [adding, setAdding] = createSignal(false);
  const [testing, setTesting] = createSignal<number>();

  const test = async (n: NotificationTarget) => {
    if (testing() === n.id) return;
    setTesting(n.id);
    try {
      await api.testNotification(n.id);
      toast(`Test sent to ${KIND_LABEL[n.kind] ?? n.kind}`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Test failed", "error", 6000);
    } finally {
      setTesting(undefined);
    }
  };

  return (
    <Panel
      title="Notifications"
      icon="bell"
      aside={
        <Show when={!adding()}>
          <button type="button" class="btn btn--primary btn--sm" onClick={() => setAdding(true)}><Icon name="plus" /> <span>Add target</span></button>
        </Show>
      }
    >
      <Show when={adding()}>
        <AddTarget onDone={(n) => { setAdding(false); if (n) void refetch(); }} />
      </Show>
      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={refetch} />}>
        <ul class="list">
          <For each={list()} fallback={<li class="panel__empty">No targets. Add Discord, Telegram or a webhook to hear when requests land.</li>}>
            {(n) => (
              <li class="list__row">
                <Icon name="bell" class="list__icon" />
                <div class="list__main">
                  <strong>{KIND_LABEL[n.kind] ?? n.kind}</strong>
                  <span class="list__sub">{n.events.length ? n.events.join(", ") : "All events"}</span>
                </div>
                <div class="list__actions">
                  <button type="button" class="btn btn--sm" aria-disabled={testing() === n.id ? "true" : undefined} onClick={() => void test(n)}>
                    <Icon name="bell" /> <span>{testing() === n.id ? "Sending…" : "Send test"}</span>
                  </button>
                  <ConfirmButton label="Remove" onConfirm={async () => { await api.deleteNotification(n.id); void refetch(); }} />
                </div>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

function AddTarget(props: { onDone: (n?: NotificationTarget) => void }): JSX.Element {
  const [kind, setKind] = createSignal<Kind>("discord");
  const [a, setA] = createSignal("");
  const [b, setB] = createSignal("");
  const [events, setEvents] = createSignal<string[]>(["available", "failed"]);
  const [busy, setBusy] = createSignal(false);
  const toggle = (id: string) => setEvents((e) => (e.indexOf(id) >= 0 ? e.filter((x) => x !== id) : e.concat(id)));

  const submit = async (ev: Event) => {
    ev.preventDefault();
    if (busy()) return;
    const config =
      kind() === "discord" ? { webhook_url: a().trim() } : kind() === "telegram" ? { bot_token: a().trim(), chat_id: b().trim() } : { url: a().trim() };
    if (!a().trim() || (kind() === "telegram" && !b().trim())) return toast("Fill in the connection details", "error");
    setBusy(true);
    try {
      const n = await api.createNotification({ kind: kind(), config: config as never, events: events() });
      toast("Notification target added — send a test to check it", "success");
      props.onDone(n);
    } catch (err) {
      toast(err instanceof Error ? err.message : "Couldn't add the target", "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="subform" onSubmit={submit} novalidate>
      <div class="segmented" role="group" aria-label="Type">
        <button type="button" class="segmented__btn" aria-pressed={kind() === "discord"} onClick={() => setKind("discord")}>Discord</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "telegram"} onClick={() => setKind("telegram")}>Telegram</button>
        <button type="button" class="segmented__btn" aria-pressed={kind() === "webhook"} onClick={() => setKind("webhook")}>Webhook</button>
      </div>
      <div class="form-grid">
        <Show when={kind() === "discord"}>
          <Field label="Discord webhook URL" placeholder="https://discord.com/api/webhooks/…" value={a()} onInput={(e) => setA(e.currentTarget.value)} spellcheck={false} data-autofocus />
        </Show>
        <Show when={kind() === "telegram"}>
          <PasswordField label="Bot token" value={a()} onInput={(e) => setA(e.currentTarget.value)} autocomplete="off" />
          <Field label="Chat ID" placeholder="-1001234567890" value={b()} onInput={(e) => setB(e.currentTarget.value)} spellcheck={false} />
        </Show>
        <Show when={kind() === "webhook"}>
          <Field label="URL" placeholder="https://example.com/hooks/reel" value={a()} onInput={(e) => setA(e.currentTarget.value)} spellcheck={false} hint="Receives a JSON POST per event" />
        </Show>
      </div>
      <div class="field">
        <span class="field__label">Events</span>
        <div class="chips" data-nav-group>
          <For each={EVENTS}>
            {(e) => (
              <button type="button" class="chip chip--toggle" aria-pressed={events().indexOf(e.id) >= 0} onClick={() => toggle(e.id)}>
                <Show when={events().indexOf(e.id) >= 0}><Icon name="check" /></Show> {e.label}
              </button>
            )}
          </For>
        </div>
        <span class="field__hint">None selected = every event</span>
      </div>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Adding…" : "Add target"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
    </form>
  );
}

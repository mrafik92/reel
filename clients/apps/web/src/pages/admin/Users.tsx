import { createResource, createSignal, For, Show, type JSX } from "solid-js";
import { Icon } from "@reel/ui";
import { api, type InviteDto, type Limits, type UserDto } from "../../api";
import { currentUser } from "../../lib/auth";
import { toast } from "../../lib/toast";
import { MIN_PASSWORD } from "../../lib/auth";
import { Field, PasswordField } from "../../components/Form";
import { ConfirmButton, DEFAULT_LIMITS, LibraryPicker, LimitsForm, limitsSummary, Panel, Select, Toggle, timeAgo } from "../../components/AdminKit";
import { ErrorState } from "../../components/States";

export default function Users(): JSX.Element {
  const [users, { refetch, mutate }] = createResource(() => api.users());
  const [invites, invitesCtl] = createResource(() => api.invites());
  const [libs] = createResource(() => api.libraries());
  const [editing, setEditing] = createSignal<number | "new" | null>(null);
  const [inviting, setInviting] = createSignal(false);
  const [fresh, setFresh] = createSignal<InviteDto>();
  const libName = (id: number) => libs()?.find((l) => l.id === id)?.name ?? `#${id}`;

  const save = (u: UserDto) => mutate((l) => (l ?? []).map((x) => (x.id === u.id ? u : x)));

  return (
    <>
      <Panel
        title="Invites"
        icon="key"
        aside={
          <Show when={!inviting()}>
            <button type="button" class="btn btn--primary btn--sm" onClick={() => setInviting(true)}><Icon name="plus" /> <span>New invite</span></button>
          </Show>
        }
      >
        <Show when={fresh()}>{(inv) => <InviteLink invite={inv()} onDismiss={() => setFresh(undefined)} />}</Show>
        <Show when={inviting()}>
          <NewInvite
            libraries={libs() ?? []}
            onDone={(inv) => {
              setInviting(false);
              if (inv) {
                setFresh(inv);
                void invitesCtl.refetch();
              }
            }}
          />
        </Show>
        <Show when={!invites.error} fallback={<ErrorState error={invites.error} onRetry={invitesCtl.refetch} />}>
          <ul class="list">
            <For each={invites()} fallback={<li class="panel__empty">No invites yet. Create one and send the link to a friend.</li>}>
              {(inv) => (
                <li class="list__row">
                  <Icon name="key" class="list__icon" />
                  <div class="list__main">
                    <strong class="mono">{inv.code}</strong>
                    <span class="list__sub">
                      <span class={`pill pill--status pill--${inv.status}`}>{inv.status}</span>{" "}
                      {inv.library_ids.length ? inv.library_ids.map(libName).join(", ") : "All libraries"} · {limitsSummary(inv.limits)}
                      {inv.expires_at ? ` · expires ${new Date(inv.expires_at * 1000).toLocaleDateString()}` : ""}
                    </span>
                  </div>
                  <div class="list__actions">
                    <Show when={inv.status === "pending" || inv.status === "active"}>
                      <CopyButton text={inv.url} />
                    </Show>
                    <ConfirmButton label="Revoke" onConfirm={async () => {
                      await api.deleteInvite(inv.id);
                      void invitesCtl.refetch();
                    }} />
                  </div>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Panel>

      <Panel
        title="Users"
        icon="user"
        aside={<button type="button" class="btn btn--sm" onClick={() => setEditing("new")}><Icon name="plus" /> <span>Add user</span></button>}
      >
        <Show when={editing() === "new"}>
          <UserEditor libraries={libs() ?? []} onDone={(u) => { setEditing(null); if (u) void refetch(); }} />
        </Show>
        <Show when={!users.error} fallback={<ErrorState error={users.error} onRetry={refetch} />}>
          <ul class="list">
            <For each={users()}>
              {(u) => (
                <li class="list__row list__row--stack" classList={{ "is-disabled": u.disabled }}>
                  <div class="list__line">
                    <span class="avatar" aria-hidden="true">{u.username.charAt(0).toUpperCase()}</span>
                    <div class="list__main">
                      <strong>
                        {u.username}
                        <Show when={u.role === "admin"}> <span class="pill pill--status pill--admin">admin</span></Show>
                        <Show when={u.disabled}> <span class="pill pill--status pill--disabled">disabled</span></Show>
                        <Show when={u.id === currentUser()?.id}> <span class="list__you">you</span></Show>
                      </strong>
                      <span class="list__sub">
                        {u.library_ids.length ? u.library_ids.map(libName).join(", ") : "All libraries"} · {limitsSummary(u.limits)} · joined {timeAgo(u.created_at)}
                      </span>
                    </div>
                    <div class="list__actions">
                      <button type="button" class="btn btn--sm" aria-expanded={editing() === u.id} onClick={() => setEditing(editing() === u.id ? null : u.id)}>
                        <Icon name="gear" /> <span>{editing() === u.id ? "Close" : "Edit"}</span>
                      </button>
                    </div>
                  </div>
                  <Show when={editing() === u.id}>
                    <UserEditor user={u} libraries={libs() ?? []} onDone={(n) => { if (n) save(n); setEditing(null); }} onDeleted={() => { setEditing(null); void refetch(); }} />
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Panel>
    </>
  );
}

function CopyButton(props: { text: string; label?: string }): JSX.Element {
  const [done, setDone] = createSignal(false);
  const copy = async () => {
    let ok = false;
    try {
      await navigator.clipboard.writeText(props.text);
      ok = true;
    } catch {
      // Older TV browsers: fall back to a hidden textarea.
      const ta = document.createElement("textarea");
      ta.value = props.text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        ok = document.execCommand("copy");
      } catch {
        ok = false;
      }
      ta.remove();
    }
    if (ok) {
      setDone(true);
      setTimeout(() => setDone(false), 2000);
    } else toast("Couldn't copy — select the link and copy it manually", "error");
  };
  return (
    <button type="button" class="btn btn--sm" onClick={() => void copy()}>
      <Icon name={done() ? "check" : "copy"} /> <span>{done() ? "Copied" : props.label ?? "Copy link"}</span>
    </button>
  );
}

function InviteLink(props: { invite: InviteDto; onDismiss: () => void }): JSX.Element {
  return (
    <div class="invite-link" role="status">
      <p class="eyebrow eyebrow--accent">Invite ready</p>
      <p>Send this link to your friend. It works once{props.invite.expires_at ? ` and expires ${new Date(props.invite.expires_at * 1000).toLocaleDateString()}` : ""}.</p>
      <div class="field__row">
        <input class="field__input field__input--mono" readonly value={props.invite.url} onFocus={(e) => e.currentTarget.select()} aria-label="Invite link" />
        <CopyButton text={props.invite.url} label="Copy" />
      </div>
      <button type="button" class="btn btn--quiet btn--sm" onClick={() => props.onDismiss()}><span>Done</span></button>
    </div>
  );
}

function NewInvite(props: { libraries: { id: number; name: string; kind: "movies" | "shows"; paths: string[]; item_count: number }[]; onDone: (inv?: InviteDto) => void }): JSX.Element {
  const [days, setDays] = createSignal(7);
  const [libIds, setLibIds] = createSignal<number[]>([]);
  const [limits, setLimits] = createSignal<Limits>({ ...DEFAULT_LIMITS });
  const [busy, setBusy] = createSignal(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    setBusy(true);
    try {
      props.onDone(await api.createInvite({ expires_in_days: days() || null, library_ids: libIds(), limits: limits() }));
    } catch (err) {
      toast(err instanceof Error ? err.message : "Couldn't create the invite", "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="subform" onSubmit={submit}>
      <div class="form-grid">
        <Select
          label="Link expires"
          value={String(days())}
          onChange={(e) => setDays(Number(e.currentTarget.value))}
          options={[{ value: 1, label: "In 1 day" }, { value: 7, label: "In 7 days" }, { value: 30, label: "In 30 days" }, { value: 0, label: "Never" }]}
        />
      </div>
      <LibraryPicker libraries={props.libraries} value={libIds()} onChange={setLibIds} />
      <details class="subform__more">
        <summary>Limits for this person</summary>
        <LimitsForm value={limits()} onChange={setLimits} />
      </details>
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined} data-autofocus><Icon name="key" /> <span>{busy() ? "Creating…" : "Create invite link"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone()}><span>Cancel</span></button>
      </div>
    </form>
  );
}

function UserEditor(props: {
  user?: UserDto;
  libraries: { id: number; name: string; kind: "movies" | "shows"; paths: string[]; item_count: number }[];
  onDone: (u?: UserDto) => void;
  onDeleted?: () => void;
}): JSX.Element {
  const u = props.user;
  const [username, setUsername] = createSignal(u?.username ?? "");
  const [password, setPassword] = createSignal("");
  const [admin, setAdmin] = createSignal(u?.role === "admin");
  const [disabled, setDisabled] = createSignal(!!u?.disabled);
  const [libIds, setLibIds] = createSignal<number[]>(u?.library_ids ?? []);
  const [limits, setLimits] = createSignal<Limits>(u ? { ...u.limits } : { ...DEFAULT_LIMITS });
  const [busy, setBusy] = createSignal(false);
  const self = () => u?.id === currentUser()?.id;

  const submit = async (e: Event) => {
    e.preventDefault();
    if (busy()) return;
    if (!u && (!username().trim() || password().length < MIN_PASSWORD)) return toast(`Username and a password of ${MIN_PASSWORD}+ characters are needed`, "error");
    setBusy(true);
    try {
      const role = admin() ? "admin" : "user";
      const res = u
        ? await api.patchUser(u.id, { role, disabled: disabled(), library_ids: libIds(), limits: limits(), password: password() || null, username: username().trim() !== u.username ? username().trim() : null })
        : await api.createUser({ username: username().trim(), password: password(), role, library_ids: libIds(), limits: limits() });
      toast(u ? `Saved ${res.username}` : `Created ${res.username}`, "success");
      props.onDone(res);
    } catch (err) {
      toast(err instanceof Error ? err.message : "Couldn't save", "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="subform" onSubmit={submit} novalidate>
      <div class="form-grid">
        <Field label="Username" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} autocapitalize="off" spellcheck={false} data-autofocus />
        <PasswordField label={u ? "New password" : "Password"} hint={u ? "Leave blank to keep the current one" : `At least ${MIN_PASSWORD} characters`} value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="new-password" />
      </div>
      <div class="toggles">
        <Toggle label="Admin" hint="Can change server settings and approve requests" checked={admin()} onChange={setAdmin} disabled={self()} />
        <Show when={u}>
          <Toggle label="Disabled" hint="Signs them out and blocks sign-in" checked={disabled()} onChange={setDisabled} disabled={self()} />
        </Show>
      </div>
      <LibraryPicker libraries={props.libraries} value={libIds()} onChange={setLibIds} />
      <LimitsForm value={limits()} onChange={setLimits} />
      <div class="actions">
        <button type="submit" class="btn btn--primary" aria-disabled={busy() ? "true" : undefined}><Icon name="check" /> <span>{busy() ? "Saving…" : u ? "Save changes" : "Create user"}</span></button>
        <button type="button" class="btn" onClick={() => props.onDone()}><span>Cancel</span></button>
        <Show when={u && !self()}>
          <ConfirmButton label="Sign out everywhere" icon="logout" onConfirm={async () => {
            await api.revokeUserSessions(u!.id);
            toast(`Signed ${u!.username} out of all devices`);
          }} />
          <ConfirmButton label="Delete user" onConfirm={async () => {
            try {
              await api.deleteUser(u!.id);
              toast(`Deleted ${u!.username}`);
              props.onDeleted?.();
            } catch (err) {
              toast(err instanceof Error ? err.message : "Couldn't delete", "error");
            }
          }} />
        </Show>
      </div>
    </form>
  );
}

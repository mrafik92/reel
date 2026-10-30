import { createSignal, For, onCleanup, Show, splitProps, type JSX, type ParentProps } from "solid-js";
import { Icon, type IconName } from "@reel/ui";
import type { Library, Limits } from "../api";

export function Panel(props: ParentProps<{ title?: string; icon?: IconName; aside?: JSX.Element; class?: string }>): JSX.Element {
  return (
    <section class={"panel" + (props.class ? " " + props.class : "")}>
      <Show when={props.title}>
        <header class="panel__head">
          <h2 class="panel__title">
            <Show when={props.icon}><Icon name={props.icon!} /></Show>
            {props.title}
          </h2>
          {props.aside}
        </header>
      </Show>
      {props.children}
    </section>
  );
}

/** Two-step destructive button: first press arms it, second confirms (no modal needed on TV). */
export function ConfirmButton(props: { label: string; confirm?: string; onConfirm: () => void | Promise<void>; icon?: IconName; class?: string }): JSX.Element {
  const [armed, setArmed] = createSignal(false);
  let t: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => t && clearTimeout(t));
  return (
    <button
      type="button"
      class={"btn btn--sm " + (armed() ? "btn--danger" : "btn--quiet") + (props.class ? " " + props.class : "")}
      onClick={() => {
        if (!armed()) {
          setArmed(true);
          t = setTimeout(() => setArmed(false), 3500);
          return;
        }
        if (t) clearTimeout(t);
        setArmed(false);
        void props.onConfirm();
      }}
      onBlur={() => setArmed(false)}
    >
      <Icon name={props.icon ?? "trash"} />
      <span>{armed() ? props.confirm ?? "Press again to confirm" : props.label}</span>
    </button>
  );
}

export function Toggle(props: { label: string; checked: boolean; onChange: (v: boolean) => void; hint?: string; disabled?: boolean }): JSX.Element {
  return (
    <button type="button" role="switch" class="toggle" aria-checked={props.checked} disabled={props.disabled} onClick={() => props.onChange(!props.checked)}>
      <span class="toggle__track"><i /></span>
      <span class="toggle__text">
        <span class="toggle__label">{props.label}</span>
        <Show when={props.hint}><span class="toggle__hint">{props.hint}</span></Show>
      </span>
    </button>
  );
}

export function Select(
  props: JSX.SelectHTMLAttributes<HTMLSelectElement> & { label: string; options: { value: string | number; label: string }[]; hint?: string },
): JSX.Element {
  const [own, rest] = splitProps(props, ["label", "options", "hint", "value"]);
  const id = `s-${Math.random().toString(36).slice(2, 8)}`;
  return (
    <div class="field">
      <label class="field__label" for={id}>{own.label}</label>
      <select id={id} class="field__input field__select" {...rest}>
        {/* `selected` per option: setting value on <select> before its options exist is lost. */}
        <For each={own.options}>{(o) => <option value={String(o.value)} selected={String(o.value) === String(own.value ?? "")}>{o.label}</option>}</For>
      </select>
      <Show when={own.hint}><span class="field__hint">{own.hint}</span></Show>
    </div>
  );
}

/** Pick zero or more libraries (empty = all libraries). */
export function LibraryPicker(props: { libraries: Library[]; value: number[]; onChange: (v: number[]) => void; label?: string }): JSX.Element {
  const toggle = (id: number) =>
    props.onChange(props.value.indexOf(id) >= 0 ? props.value.filter((x) => x !== id) : props.value.concat(id));
  return (
    <div class="field">
      <span class="field__label">{props.label ?? "Library access"}</span>
      <div class="chips" data-nav-group>
        <button type="button" class="chip chip--toggle" aria-pressed={props.value.length === 0} onClick={() => props.onChange([])}>
          <Show when={props.value.length === 0}><Icon name="check" /></Show> All libraries
        </button>
        <For each={props.libraries}>
          {(l) => (
            <button type="button" class="chip chip--toggle" aria-pressed={props.value.indexOf(l.id) >= 0} onClick={() => toggle(l.id)}>
              <Show when={props.value.indexOf(l.id) >= 0}><Icon name="check" /></Show> {l.name}
            </button>
          )}
        </For>
      </div>
    </div>
  );
}

const numOrNull = (v: string): number | null => (v.trim() === "" ? null : Math.max(0, Math.floor(Number(v)) || 0));

/** Per-user limits (SPEC §8). Blank = unlimited. */
export function LimitsForm(props: { value: Limits; onChange: (v: Limits) => void }): JSX.Element {
  const set = <K extends keyof Limits>(k: K, v: Limits[K]) => props.onChange({ ...props.value, [k]: v });
  const num = (label: string, k: "max_streams" | "max_transcodes" | "max_bitrate_kbps" | "request_quota_movies" | "request_quota_tv", hint?: string) => (
    <div class="field field--num">
      <label class="field__label">{label}</label>
      <input
        class="field__input"
        type="number"
        min="0"
        inputmode="numeric"
        placeholder="Unlimited"
        value={props.value[k] ?? ""}
        onInput={(e) => set(k, numOrNull(e.currentTarget.value))}
      />
      <Show when={hint}><span class="field__hint">{hint}</span></Show>
    </div>
  );
  return (
    <div class="limits">
      <div class="form-grid">
        {num("Simultaneous streams", "max_streams")}
        {num("Simultaneous transcodes", "max_transcodes")}
        {num("Max bitrate (kbps)", "max_bitrate_kbps", "Forces a lower quality above this")}
        {num("Film requests", "request_quota_movies", "Per quota window")}
        {num("Series requests", "request_quota_tv", "Per quota window")}
        <div class="field field--num">
          <label class="field__label">Quota window (days)</label>
          <input class="field__input" type="number" min="1" value={props.value.quota_window_days} onInput={(e) => set("quota_window_days", Math.max(1, Number(e.currentTarget.value) || 7))} />
        </div>
      </div>
      <Toggle label="Auto-approve requests" hint="Their requests download immediately" checked={props.value.auto_approve} onChange={(v) => set("auto_approve", v)} />
    </div>
  );
}

export const DEFAULT_LIMITS: Limits = {
  auto_approve: false, max_bitrate_kbps: null, max_streams: 2, max_transcodes: 1,
  quota_window_days: 7, request_quota_movies: 10, request_quota_tv: 3,
};

export function limitsSummary(l: Limits): string {
  const parts: string[] = [];
  parts.push(l.max_streams != null ? `${l.max_streams} stream${l.max_streams === 1 ? "" : "s"}` : "unlimited streams");
  if (l.max_transcodes != null) parts.push(`${l.max_transcodes} transcode${l.max_transcodes === 1 ? "" : "s"}`);
  if (l.max_bitrate_kbps) parts.push(`≤ ${Math.round(l.max_bitrate_kbps / 1000)} Mbps`);
  if (l.auto_approve) parts.push("auto-approve");
  return parts.join(" · ");
}

export function timeAgo(unix: number): string {
  const d = Date.now() / 1000 - unix;
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  const n = Math.round(d / 86400);
  return n === 1 ? "yesterday" : `${n} days ago`;
}

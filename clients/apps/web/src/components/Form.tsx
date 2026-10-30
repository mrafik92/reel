import { createSignal, onCleanup, Show, splitProps, type JSX } from "solid-js";
import { Icon } from "@reel/ui";

/** Labelled text input sized for remotes: big target, visible label, error slot. */
export function Field(
  props: JSX.InputHTMLAttributes<HTMLInputElement> & { label: string; hint?: string; error?: string | false },
): JSX.Element {
  const [own, rest] = splitProps(props, ["label", "hint", "error", "class"]);
  const id = `f-${Math.random().toString(36).slice(2, 8)}`;
  return (
    <div class={"field" + (own.class ? " " + own.class : "")} classList={{ "has-error": !!own.error }}>
      <label class="field__label" for={id}>{own.label}</label>
      <input id={id} class="field__input" aria-invalid={own.error ? "true" : undefined} {...rest} />
      <Show when={own.error || own.hint}>
        <span class={own.error ? "field__error" : "field__hint"}>{own.error || own.hint}</span>
      </Show>
    </div>
  );
}

/** Password with a show/hide toggle (typing on a TV keyboard is error-prone). */
export function PasswordField(
  props: Omit<JSX.InputHTMLAttributes<HTMLInputElement>, "type"> & { label: string; hint?: string; error?: string | false },
): JSX.Element {
  const [show, setShow] = createSignal(false);
  const [own, rest] = splitProps(props, ["label", "hint", "error"]);
  const id = `p-${Math.random().toString(36).slice(2, 8)}`;
  return (
    <div class="field" classList={{ "has-error": !!own.error }}>
      <label class="field__label" for={id}>{own.label}</label>
      <div class="field__row">
        <input id={id} class="field__input" type={show() ? "text" : "password"} aria-invalid={own.error ? "true" : undefined} {...rest} />
        <button type="button" class="iconbtn field__eye" aria-label={show() ? "Hide password" : "Show password"} aria-pressed={show()} onClick={() => setShow((v) => !v)}>
          <Icon name={show() ? "eyeoff" : "eye"} />
        </button>
      </div>
      <Show when={own.error || own.hint}>
        <span class={own.error ? "field__error" : "field__hint"}>{own.error || own.hint}</span>
      </Show>
    </div>
  );
}

/** Seconds-remaining countdown for 429 lockouts. */
export function useCountdown(): [() => number, (s: number) => void] {
  const [left, setLeft] = createSignal(0);
  let t: ReturnType<typeof setInterval> | undefined;
  onCleanup(() => t && clearInterval(t));
  const start = (s: number) => {
    setLeft(Math.ceil(s));
    if (t) clearInterval(t);
    t = setInterval(() => {
      setLeft((v) => {
        if (v <= 1 && t) clearInterval(t);
        return Math.max(0, v - 1);
      });
    }, 1000);
  };
  return [left, start];
}

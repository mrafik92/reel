import { createSignal, onCleanup, Show, type JSX } from "solid-js";
import { formatClock, Icon, type IconName } from "@reel/ui";
import { TV } from "../../lib/mode";
import { volumeIcon } from "../../lib/playerChrome";
import "../../styles/player.css";

/**
 * Player control kit — shared by the library/VOD player and the cast "Playing on…" view:
 * the scrubber (hover/seek tooltip with a trickplay slot, buffered range, keyboard seek mode
 * with accelerating steps), labelled icon buttons, the transport cluster and a volume slider.
 */

// Elapsed · remaining vs. total on the right-hand clock: one choice for every player.
const REM_KEY = "reel.player.showRemaining";
const [showRemaining, setShowRemaining] = createSignal(readRemaining());
function readRemaining(): boolean {
  try {
    return localStorage.getItem(REM_KEY) !== "0";
  } catch {
    return true;
  }
}
export function toggleRemaining(): void {
  const v = !showRemaining();
  setShowRemaining(v);
  try {
    localStorage.setItem(REM_KEY, v ? "1" : "0");
  } catch {
    /* ignore */
  }
}

export interface ScrubberApi {
  /** Move the preview by `delta` s (enters seek mode). `quick`: commit soon (buttons). */
  nudge(delta: number, quick?: boolean): void;
  /** Leave seek mode without seeking. True if there was something to cancel. */
  cancel(): boolean;
  active(): boolean;
  el(): HTMLDivElement | undefined;
}

const fmtDelta = (d: number) => `${d < 0 ? "−" : "+"}${formatClock(Math.abs(d))}`;

export function Scrubber(props: {
  position: number;
  duration: number;
  buffered?: number;
  onSeek: (t: number) => void;
  /** Preview while dragging / seeking with keys (null = none). */
  onPreview?: (t: number | null) => void;
  onActivity?: () => void;
  /** Trickplay frame for a time — none yet on the server; the tooltip grows a frame when it exists. */
  thumbAt?: (t: number) => string | undefined;
  api?: (a: ScrubberApi) => void;
  label?: string;
}): JSX.Element {
  let bar!: HTMLDivElement;
  let tip: HTMLDivElement | undefined;
  const [preview, setPreviewRaw] = createSignal<number | null>(null);
  /** Where keyboard seeking started (shows "+1:30"); null for pointer drags. */
  const [origin, setOrigin] = createSignal<number | null>(null);
  const [hover, setHover] = createSignal<number | null>(null);
  const [dragging, setDragging] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => timer && clearTimeout(timer));

  const setPreview = (v: number | null) => {
    setPreviewRaw(v);
    props.onPreview?.(v);
  };
  const dur = () => props.duration || 0;
  const clamp = (t: number) => Math.max(0, Math.min(Math.max(0, dur() - 1), t));
  const shown = () => preview() ?? props.position;
  const pct = (t: number) => (dur() ? Math.min(100, (t / dur()) * 100) : 0);

  const commit = () => {
    if (timer) clearTimeout(timer);
    timer = undefined;
    const p = preview();
    setOrigin(null);
    if (p != null) props.onSeek(p);
    setPreview(null);
  };
  const arm = (ms: number) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(commit, ms);
  };
  /** Seek mode: the preview moves, playback carries on, OK (or a pause) commits. */
  const nudge = (delta: number, quick = false) => {
    if (!dur()) return;
    if (preview() == null) setOrigin(props.position);
    setPreview(clamp((preview() ?? props.position) + delta));
    props.onActivity?.();
    arm(quick ? 450 : TV ? 2600 : 900);
  };
  const cancel = () => {
    if (preview() == null) return false;
    if (timer) clearTimeout(timer);
    timer = undefined;
    setOrigin(null);
    setPreview(null);
    return true;
  };
  props.api?.({ nudge, cancel, active: () => preview() != null, el: () => bar });

  // Arrow steps accelerate while held (key repeat) or pressed in quick succession.
  let streak = 0;
  let last = 0;
  const step = (dir: number) => {
    const now = Date.now();
    streak = now - last < 420 ? streak + 1 : 0;
    last = now;
    const s = streak > 26 ? 120 : streak > 14 ? 60 : streak > 5 ? 30 : 10;
    nudge(dir * s);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      e.stopPropagation();
      step(e.key === "ArrowLeft" ? -1 : 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      e.stopPropagation();
      if (preview() != null) commit();
      else {
        // OK enters seek mode: the tooltip shows, ←/→ move it, OK again jumps there.
        setOrigin(props.position);
        setPreview(props.position);
        props.onActivity?.();
        arm(6000);
      }
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      nudge((e.key === "Home" ? 0 : dur() - 1) - (preview() ?? props.position));
    }
  };

  const at = (e: PointerEvent) => {
    const r = bar.getBoundingClientRect();
    return clamp(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * dur());
  };
  const tipTime = () => (preview() != null ? preview()! : hover());
  const thumb = () => {
    const t = tipTime();
    return t != null ? props.thumbAt?.(t) : undefined;
  };
  /** Tooltip x in px, kept inside the bar. */
  const tipLeft = () => {
    const t = tipTime();
    if (t == null || !bar) return 0;
    const w = bar.clientWidth;
    const half = (tip?.offsetWidth ?? 80) / 2;
    const x = (pct(t) / 100) * w;
    return Math.max(half - 8, Math.min(w - half + 8, x));
  };

  return (
    <div class="pscrub" classList={{ "is-seeking": preview() != null, "is-dragging": dragging() }}>
      <span class="pscrub__time">{formatClock(shown())}</span>
      <div
        ref={bar}
        class="pscrub__bar"
        role="slider"
        tabindex="0"
        data-focusable
        data-nav-capture="left right"
        aria-label={props.label ?? "Seek"}
        aria-valuemin={0}
        aria-valuemax={Math.round(dur())}
        aria-valuenow={Math.round(shown())}
        aria-valuetext={`${formatClock(shown())} of ${formatClock(dur())}`}
        onKeyDown={onKey}
        onBlur={() => preview() != null && origin() != null && commit()}
        onPointerDown={(e) => {
          bar.setPointerCapture?.(e.pointerId);
          setDragging(true);
          setOrigin(null);
          setPreview(at(e));
          props.onActivity?.();
        }}
        onPointerMove={(e) => {
          if (e.pointerType === "mouse") setHover(at(e));
          if (dragging()) setPreview(at(e));
        }}
        onPointerUp={(e) => {
          if (!dragging()) return;
          setDragging(false);
          setPreview(at(e));
          commit();
        }}
        onPointerCancel={() => {
          setDragging(false);
          cancel();
        }}
        onPointerLeave={() => setHover(null)}
      >
        <div class="pscrub__track">
          <i class="pscrub__buf" style={{ width: `${pct(props.buffered ?? 0)}%` }} />
          <Show when={hover() != null && preview() == null}>
            <i class="pscrub__ghost" style={{ width: `${pct(hover()!)}%` }} />
          </Show>
          <i class="pscrub__fill" style={{ width: `${pct(shown())}%` }} />
        </div>
        <span class="pscrub__thumb" style={{ left: `${pct(shown())}%` }} />
        <Show when={tipTime() != null}>
          <div ref={tip} class="pscrub__tip" classList={{ "has-frame": !!thumb() }} style={{ left: `${tipLeft()}px` }} aria-hidden="true">
            <Show when={thumb()}>
              {(src) => (
                <span class="pscrub__frame">
                  <img src={src()} alt="" />
                </span>
              )}
            </Show>
            <span class="pscrub__tipline">
              <strong>{formatClock(tipTime()!)}</strong>
              <Show when={origin() != null && preview() != null && Math.abs(preview()! - origin()!) >= 1}>
                <small>{fmtDelta(preview()! - origin()!)}</small>
              </Show>
            </span>
          </div>
        </Show>
      </div>
      <button
        type="button"
        class="pscrub__time pscrub__time--end"
        tabindex="-1"
        title={showRemaining() ? "Show total length" : "Show time left"}
        onClick={toggleRemaining}
      >
        {showRemaining() ? `−${formatClock(Math.max(0, dur() - shown()))}` : formatClock(dur())}
      </button>
    </div>
  );
}

/**
 * Round icon button of a fixed width; its name slides in as a label on hover / focus. The current
 * choice goes in that label (`hint`), not next to the icon, so a row of these never changes width.
 */
export function CtlButton(props: {
  icon: IconName;
  label: string;
  /** Current choice, shown after the name in the label: "Audio & subtitles · English SDH". */
  hint?: string;
  /** Words next to the icon, for rows that ask for them (Next episode, the phone's action row). */
  text?: string;
  /** A small outlined badge instead of the icon, e.g. the size that is playing: "1080p". */
  badge?: string;
  /** Compact current choice shown next to the icon, e.g. "EN · 5.1" (cast remote). */
  value?: string;
  /** Small marker after the value (e.g. "Auto"). */
  tag?: string;
  pressed?: boolean;
  active?: boolean;
  disabled?: boolean;
  class?: string;
  ref?: (el: HTMLButtonElement) => void;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      ref={props.ref}
      class={"ctl" + (props.class ? " " + props.class : "")}
      classList={{ "has-value": !!props.value, "has-text": !!props.text, "has-badge": !!props.badge, "is-active": !!props.active }}
      aria-label={props.hint || props.value ? `${props.label}: ${props.hint || props.value}` : props.label}
      aria-pressed={props.pressed === undefined ? undefined : props.pressed}
      disabled={props.disabled}
      onClick={() => props.onClick()}
    >
      <Show when={props.badge} fallback={<Icon name={props.icon} />}>
        <span class="ctl__badge">{props.badge}</span>
      </Show>
      <Show when={props.text}>
        <span class="ctl__text">{props.text}</span>
      </Show>
      <Show when={props.value}>
        <span class="ctl__val">
          {props.value}
          <Show when={props.tag}><i class="ctl__tag">{props.tag}</i></Show>
        </span>
      </Show>
      <span class="ctl__tip" aria-hidden="true">
        {props.label}
        <Show when={props.hint}><small>{props.hint}</small></Show>
      </span>
    </button>
  );
}

/** −10 · play/pause · +30: the centred transport. */
export function Transport(props: {
  playing: boolean;
  onToggle: () => void;
  onBack?: () => void;
  onForward?: () => void;
  back?: number;
  forward?: number;
  playRef?: (el: HTMLButtonElement) => void;
  autofocus?: boolean;
}): JSX.Element {
  return (
    <div class="ptx" data-nav-group>
      <Show when={props.onBack}>
        <button type="button" class="ptx__skip" aria-label={`Back ${props.back ?? 10} seconds`} onClick={() => props.onBack!()}>
          <Icon name="replay" />
          <span class="ptx__secs">{props.back ?? 10}</span>
          <span class="ctl__tip" aria-hidden="true">Back {props.back ?? 10}s</span>
        </button>
      </Show>
      <button
        type="button"
        class="ptx__play"
        classList={{ "is-playing": props.playing }}
        ref={props.playRef}
        data-autofocus={props.autofocus ? "" : undefined}
        data-nav-default
        aria-label={props.playing ? "Pause" : "Play"}
        onClick={() => props.onToggle()}
      >
        <Icon name={props.playing ? "pause" : "play"} />
      </button>
      <Show when={props.onForward}>
        <button type="button" class="ptx__skip" aria-label={`Forward ${props.forward ?? 30} seconds`} onClick={() => props.onForward!()}>
          <Icon name="skip" />
          <span class="ptx__secs">{props.forward ?? 30}</span>
          <span class="ctl__tip" aria-hidden="true">Forward {props.forward ?? 30}s</span>
        </button>
      </Show>
    </div>
  );
}

/** Mute + slider (0..1). The icon follows the level; the slider opens on hover / focus; ←/→ step 5 %. */
export function Volume(props: {
  value: number | null;
  muted?: boolean;
  onChange: (v: number) => void;
  onMute: () => void;
  label?: string;
}): JSX.Element {
  let track!: HTMLDivElement;
  const level = () => (props.muted ? 0 : props.value ?? 0);
  const set = (v: number) => props.onChange(Math.max(0, Math.min(1, Math.round(v * 20) / 20)));
  const fromPointer = (e: PointerEvent) => {
    const r = track.getBoundingClientRect();
    set((e.clientX - r.left) / r.width);
  };
  return (
    <div class="pvol" classList={{ "is-unknown": props.value == null }}>
      <button type="button" class="ctl" aria-label={level() === 0 ? "Unmute" : "Mute"} onClick={() => props.onMute()}>
        <Icon name={volumeIcon(level())} />
        <span class="ctl__tip" aria-hidden="true">{level() === 0 ? "Unmute" : "Mute"} (M)</span>
      </button>
      <div
        class="pvol__bar"
        role="slider"
        tabindex="0"
        data-focusable
        data-nav-capture="left right"
        aria-label={props.label ?? "Volume"}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={props.value == null ? undefined : Math.round(level() * 100)}
        aria-valuetext={props.value == null ? "Unknown" : `${Math.round(level() * 100)}%`}
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
            e.preventDefault();
            e.stopPropagation();
            set((props.value ?? 0.5) * (props.muted ? 0 : 1) + (e.key === "ArrowLeft" ? -0.05 : 0.05));
          }
        }}
        onPointerDown={(e) => {
          (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
          fromPointer(e);
        }}
        onPointerMove={(e) => e.buttons && fromPointer(e)}
      >
        <div class="pvol__track" ref={track}>
          <i style={{ width: `${level() * 100}%` }} />
          <span class="pvol__knob" style={{ left: `${level() * 100}%` }} />
        </div>
      </div>
    </div>
  );
}

/** Centre flash for keyboard / click feedback ("+10s", "Volume 60 %", play/pause glyph). */
export function useOsd(): [() => { icon?: IconName; text?: string; key: number } | undefined, (o: { icon?: IconName; text?: string }) => void] {
  const [osd, setOsd] = createSignal<{ icon?: IconName; text?: string; key: number }>();
  let t: ReturnType<typeof setTimeout> | undefined;
  let n = 0;
  onCleanup(() => t && clearTimeout(t));
  return [
    osd,
    (o) => {
      setOsd({ ...o, key: ++n });
      if (t) clearTimeout(t);
      t = setTimeout(() => setOsd(undefined), 750);
    },
  ];
}

export function Osd(props: { osd?: { icon?: IconName; text?: string; key: number } }): JSX.Element {
  return (
    <Show when={props.osd} keyed>
      {(o) => (
        <div class="posd" classList={{ "has-text": !!o.text }} aria-hidden="true">
          <Show when={o.icon}><Icon name={o.icon!} /></Show>
          <Show when={o.text}><span>{o.text}</span></Show>
        </div>
      )}
    </Show>
  );
}

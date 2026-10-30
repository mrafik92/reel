import { createSignal, onCleanup, Show, type JSX } from "solid-js";
import { formatClock, Icon, mediaUrl, Sheet, type IconName } from "@reel/ui";
import "../../styles/casting.css";
import { castAudio, castRetry, castSubtitle, type ActiveCast, type CastTrack } from "../../lib/casting";

/**
 * "Playing on {device}": the page becomes the remote while a TV plays. Big art, the
 * receiver's own clock mirrored into the scrubber, and the same Audio / Subtitles pickers
 * as the player (switching re-loads the TV when it can't swap tracks in place).
 */
export function CastRemote(props: {
  cast: ActiveCast;
  eyebrow?: string;
  /** Audio/subtitle lists when the receiver didn't give us one (DLNA). */
  audio?: CastTrack[];
  subtitles?: CastTrack[];
  onBack: () => void;
  /** Open "Play on…" to switch device or come back to this screen. */
  onDevices: () => void;
  onStop: () => void;
}): JSX.Element {
  const r = () => props.cast.remote;
  const meta = () => props.cast.source.meta;
  const live = () => !!meta().live;
  const [scrub, setScrub] = createSignal<number | null>(null);
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [stopping, setStopping] = createSignal(false);
  let bar!: HTMLDivElement;
  let commit: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => commit && clearTimeout(commit));

  const audioList = () => r().stream()?.audio ?? props.audio ?? props.cast.source.audio ?? [];
  const subList = () => r().stream()?.subtitles ?? props.subtitles ?? props.cast.source.subtitles ?? [];
  const audioText = () => audioList().find((a) => a.index === props.cast.audio())?.title ?? "Default";
  const subText = () => subList().find((s) => s.index === props.cast.sub())?.title ?? "Off";

  const dur = () => r().duration() || meta().duration || 0;
  const shown = () => scrub() ?? r().position();
  const pct = () => (dur() ? Math.min(100, (shown() / dur()) * 100) : 0);
  const playing = () => r().state() === "playing";
  const waiting = () => r().state() === "connecting" || r().state() === "buffering" || props.cast.busy();

  const seekBy = (d: number) => {
    if (live()) return;
    const base = scrub() ?? r().position();
    setScrub(Math.max(0, Math.min((dur() || base + d) - 1, base + d)));
    commitLater();
  };
  const commitLater = () => {
    if (commit) clearTimeout(commit);
    commit = setTimeout(() => {
      const s = scrub();
      if (s != null) r().seek(s);
      setScrub(null);
    }, 450);
  };
  const toggle = () => (playing() ? r().pause() : r().play());

  let held = 0;
  const onBarKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      held = e.repeat ? held + 1 : 0;
      const step = held > 20 ? 60 : held > 8 ? 30 : 10;
      seekBy(e.key === "ArrowLeft" ? -step : step);
    } else if (e.key === "Enter") {
      e.preventDefault();
      toggle();
    }
  };
  const onBarPointer = (e: PointerEvent) => {
    const b = bar.getBoundingClientRect();
    const p = Math.max(0, Math.min(1, (e.clientX - b.left) / b.width));
    setScrub(p * dur());
    if (e.type === "pointerup") commitLater();
  };

  // Volume: 0..1 in 5 % steps; unknown until the receiver reports (or we set) it.
  const vol = () => r().volume();
  let lastVol = 0.5;
  const setVol = (v: number) => {
    const n = Math.max(0, Math.min(1, Math.round(v * 20) / 20));
    if (n > 0) lastVol = n;
    r().setVolume(n);
  };
  const onVolKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      setVol((vol() ?? 0.5) + (e.key === "ArrowLeft" ? -0.05 : 0.05));
    }
  };
  let volBar!: HTMLDivElement;
  const onVolPointer = (e: PointerEvent) => {
    const b = volBar.getBoundingClientRect();
    setVol((e.clientX - b.left) / b.width);
  };

  const status = (): string => {
    if (props.cast.busy()) return "Switching on the TV…";
    switch (r().state()) {
      case "connecting": return "Starting on the TV…";
      case "buffering": return "Loading on the TV…";
      case "paused": return "Paused";
      case "idle": return "Ready";
      default: return "";
    }
  };
  const kindIcon = (): IconName => (r().kind === "airplay" ? "airplay" : r().kind === "chromecast" ? "cast" : "tv");
  const art = () => mediaUrl(meta().poster, 500);
  const backdrop = () => mediaUrl(meta().backdrop ?? meta().poster, 1280);

  return (
    <section class="castview" aria-label={`Playing on ${r().device()}`} classList={{ "is-playing": playing(), "is-waiting": waiting() }}>
      <div class="castview__bg" aria-hidden="true">
        <Show when={backdrop()}>{(b) => <img src={b()} alt="" />}</Show>
      </div>

      <header class="castview__top">
        <button type="button" class="iconbtn" aria-label="Back" onClick={() => props.onBack()}>
          <Icon name="back" />
        </button>
        <button type="button" class="castview__device" onClick={() => props.onDevices()} aria-label={`Playing on ${r().device()} — change`}>
          <Icon name={kindIcon()} />
          <span>{r().device()}</span>
          <Icon name="down" class="castview__chev" />
        </button>
      </header>

      <div class="castview__body">
        <div class="castview__art" aria-hidden="true">
          <Show when={art()} fallback={<div class="castview__art-empty"><Icon name={live() ? "live" : "film"} /></div>}>
            {(a) => <img src={a()} alt="" />}
          </Show>
          <span class="castview__beam" />
        </div>

        <div class="castview__main">
          <p class="castview__on">
            <span class="castview__dot" aria-hidden="true" />
            Playing on <strong>{r().device()}</strong>
          </p>
          <Show when={props.eyebrow ?? meta().eyebrow}>
            <p class="eyebrow castview__eyebrow">{props.eyebrow ?? meta().eyebrow}</p>
          </Show>
          <h1 class="castview__title">{meta().title}</h1>
          <p class="castview__status" role="status" aria-live="polite">
            <Show when={waiting()}><span class="spinner" aria-hidden="true" /></Show>
            {status()}
          </p>

          <Show
            when={!live()}
            fallback={
              <div class="castview__live">
                <span class="castview__livepill">Live</span>
                <span>Seeking isn't available on live TV</span>
              </div>
            }
          >
            <div class="scrub castview__scrub">
              <span class="scrub__time">{formatClock(shown())}</span>
              <div
                ref={bar}
                class="scrub__bar"
                role="slider"
                tabindex="0"
                data-focusable
                data-nav-capture="left right"
                aria-label="Seek on the TV"
                aria-valuemin={0}
                aria-valuemax={Math.round(dur())}
                aria-valuenow={Math.round(shown())}
                aria-valuetext={formatClock(shown())}
                onKeyDown={onBarKey}
                onPointerDown={(e) => {
                  bar.setPointerCapture?.(e.pointerId);
                  onBarPointer(e);
                }}
                onPointerMove={(e) => e.buttons && onBarPointer(e)}
                onPointerUp={onBarPointer}
              >
                <div class="scrub__track">
                  <i class="scrub__fill" style={{ width: `${pct()}%` }} />
                </div>
                <span class="scrub__thumb" style={{ left: `${pct()}%` }} />
                <Show when={scrub() != null}>
                  <span class="scrub__bubble" style={{ left: `${pct()}%` }}>{formatClock(scrub()!)}</span>
                </Show>
              </div>
              <span class="scrub__time scrub__time--end">-{formatClock(Math.max(0, dur() - shown()))}</span>
            </div>
          </Show>

          <div class="castview__transport" data-nav-group>
            <Show when={!live()}>
              <button type="button" class="iconbtn" aria-label="Back 10 seconds" onClick={() => seekBy(-10)}>
                <Icon name="replay" />
                <small>10</small>
              </button>
            </Show>
            <button
              type="button"
              class="iconbtn iconbtn--xl castview__play"
              aria-label={playing() ? "Pause" : "Play"}
              data-autofocus
              onClick={toggle}
            >
              <Icon name={playing() ? "pause" : "play"} />
            </button>
            <Show when={!live()}>
              <button type="button" class="iconbtn" aria-label="Forward 30 seconds" onClick={() => seekBy(30)}>
                <Icon name="skip" />
                <small>30</small>
              </button>
            </Show>
          </div>

          <div class="castview__opts" data-nav-group>
            <Show when={audioList().length > 1}>
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("audio")}>
                <Icon name="audio" /> <span class="btn__value">{audioText()}</span>
              </button>
            </Show>
            <Show when={subList().length}>
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("sub")}>
                <Icon name="cc" /> <span class="btn__value">{subText()}</span>
              </button>
            </Show>
            <Show when={r().canVolume}>
            <div class="castvol">
              <button
                type="button"
                class="iconbtn"
                aria-label={vol() === 0 ? "Unmute" : "Mute"}
                onClick={() => setVol(vol() === 0 ? lastVol : 0)}
              >
                <Icon name={vol() === 0 ? "mute" : "volume"} />
              </button>
              <div
                ref={volBar}
                class="castvol__bar"
                role="slider"
                tabindex="0"
                data-focusable
                data-nav-capture="left right"
                aria-label="TV volume"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={vol() == null ? undefined : Math.round(vol()! * 100)}
                aria-valuetext={vol() == null ? "Unknown" : `${Math.round(vol()! * 100)}%`}
                onKeyDown={onVolKey}
                onPointerDown={(e) => {
                  volBar.setPointerCapture?.(e.pointerId);
                  onVolPointer(e);
                }}
                onPointerMove={(e) => e.buttons && onVolPointer(e)}
              >
                <div class="castvol__track">
                  <i style={{ width: `${(vol() ?? 0) * 100}%` }} classList={{ "is-unknown": vol() == null }} />
                </div>
              </div>
            </div>
            </Show>
            <button
              type="button"
              class="btn castview__stop"
              aria-busy={stopping()}
              onClick={() => {
                setStopping(true);
                props.onStop();
              }}
            >
              <Icon name="stop" /> <span>{stopping() ? "Stopping…" : "Stop casting"}</span>
            </button>
          </div>

          <Show when={props.cast.error()}>
            <p class="castview__error" role="alert">
              <Icon name="alert" /> {props.cast.error()}
              <button type="button" class="sheet__link" onClick={() => castRetry()}>Try again</button>
            </p>
          </Show>
        </div>
      </div>

      <Show when={picker() === "audio"}>
        <Sheet
          title="Audio on the TV"
          selected={props.cast.audio() ?? -1}
          options={audioList().map((a) => ({ value: a.index, title: a.title, detail: a.detail }))}
          onSelect={(v) => castAudio(v)}
          onClose={() => setPicker(null)}
        />
      </Show>
      <Show when={picker() === "sub"}>
        <Sheet
          title="Subtitles on the TV"
          selected={props.cast.sub()}
          options={[{ value: -1, title: "Off" } as { value: number; title: string; detail?: string }].concat(
            subList().map((s) => ({ value: s.index, title: s.title, detail: s.detail })),
          )}
          onSelect={(v) => castSubtitle(v)}
          onClose={() => setPicker(null)}
        />
      </Show>
    </section>
  );
}

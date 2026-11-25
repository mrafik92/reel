import { createSignal, Show, type JSX } from "solid-js";
import { Icon, iso1, mediaUrl, Sheet, type IconName } from "@reel/ui";
import { CtlButton, Scrubber, Transport, Volume, type ScrubberApi } from "../player/Controls";
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
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [stopping, setStopping] = createSignal(false);
  const [preview, setPreview] = createSignal<number | null>(null);
  let scrubApi: ScrubberApi | undefined;

  const audioList = () => r().stream()?.audio ?? props.audio ?? props.cast.source.audio ?? [];
  const subList = () => r().stream()?.subtitles ?? props.subtitles ?? props.cast.source.subtitles ?? [];
  const audioText = () => audioList().find((a) => a.index === props.cast.audio())?.title ?? "Default";
  const subText = () => subList().find((s) => s.index === props.cast.sub())?.title ?? "Off";
  /** Compact: "EN", "EN SDH", "Off". */
  const short = (t: CastTrack | undefined, none: string) =>
    t ? (iso1(t.language).toUpperCase() || t.title.slice(0, 3).toUpperCase()) + (/\bSDH\b/.test(t.title) ? " SDH" : "") : none;

  const dur = () => r().duration() || meta().duration || 0;
  const playing = () => r().state() === "playing";
  const waiting = () => r().state() === "connecting" || r().state() === "buffering" || props.cast.busy();
  const seekBy = (d: number) => !live() && scrubApi?.nudge(d, true);
  const toggle = () => (playing() ? r().pause() : r().play());
  // Volume remembers the last level so Mute toggles back to it.
  let lastVol = 0.5;
  const setVol = (v: number) => {
    if (v > 0) lastVol = v;
    r().setVolume(v);
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
                <span>Seeking isn’t available on live TV</span>
              </div>
            }
          >
            <div class="castview__scrub" classList={{ "is-seeking": preview() != null }}>
              <Scrubber
                position={r().position()}
                duration={dur()}
                onSeek={(t) => r().seek(t)}
                onPreview={setPreview}
                label="Seek on the TV"
                api={(a) => (scrubApi = a)}
              />
            </div>
          </Show>

          <div class="castview__controls">
            <Transport
              playing={playing()}
              onToggle={toggle}
              onBack={live() ? undefined : () => seekBy(-10)}
              onForward={live() ? undefined : () => seekBy(30)}
              autofocus
            />
            <div class="castview__opts" data-nav-group>
              <Show when={audioList().length > 1}>
                <CtlButton
                  icon="audio"
                  label={`Audio on the TV · ${audioText()}`}
                  value={short(audioList().find((a) => a.index === props.cast.audio()), "Default")}
                  onClick={() => setPicker("audio")}
                />
              </Show>
              <Show when={subList().length}>
                <CtlButton
                  icon="cc"
                  label={`Subtitles on the TV · ${subText()}`}
                  value={short(subList().find((x) => x.index === props.cast.sub()), "Off")}
                  active={props.cast.sub() >= 0}
                  onClick={() => setPicker("sub")}
                />
              </Show>
              <Show when={r().canVolume}>
                <div class="pvol is-open">
                  <Volume value={r().volume()} onChange={setVol} onMute={() => setVol((r().volume() ?? 0) === 0 ? lastVol : 0)} label="TV volume" />
                </div>
              </Show>
            </div>
          </div>
          <button
            type="button"
            class="btn btn--sm castview__stop"
            aria-busy={stopping()}
            onClick={() => {
              setStopping(true);
              props.onStop();
            }}
          >
            <Icon name="stop" /> <span>{stopping() ? "Stopping…" : "Stop casting"}</span>
          </button>

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

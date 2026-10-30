import { createResource, createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import {
  HlsJsAdapter, Icon, Sheet, audioLabel, channelsLabel, episodeCode, focusElement, formatClock, mediaUrl,
  onBack, onMediaKey, subtitleLabel, type ItemDetail, type MediaFile,
} from "@reel/ui";
import { api } from "../api";
import { getTrackPrefs, setTrackPrefs, useProgressReporter } from "../lib/playback";
import { ErrorState } from "../components/States";

export default function Player(): JSX.Element {
  const params = useParams<{ id: string }>();
  const [item, { refetch }] = createResource(() => Number(params.id), (id) => api.item(id));
  return (
    <div class="player">
      <Show when={!item.error} fallback={<ErrorState error={item.error} onRetry={refetch} />}>
        <Show when={item()} fallback={<div class="player__loading"><span class="spinner" /></div>}>
          {(it) => <PlayerView item={it()} />}
        </Show>
      </Show>
    </div>
  );
}

const HIDE_AFTER = 4000;

function PlayerView(props: { item: ItemDetail }): JSX.Element {
  const navigate = useNavigate();
  const [search] = useSearchParams<{ file?: string; start?: string; t?: string }>();
  const file: MediaFile | undefined =
    props.item.files.find((f) => String(f.id) === search.file) ?? props.item.files[0];

  const [playing, setPlaying] = createSignal(false);
  const [buffering, setBuffering] = createSignal(true);
  const [pos, setPos] = createSignal(0);
  const [dur, setDur] = createSignal(file?.duration_s ?? 0);
  const [buffered, setBuffered] = createSignal(0);
  const [visible, setVisible] = createSignal(true);
  const [error, setError] = createSignal<string>();
  const [scrub, setScrub] = createSignal<number | null>(null);
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [series, setSeries] = createSignal<string>();

  const prefs = getTrackPrefs(props.item.id);
  const [audio, setAudio] = createSignal(prefs.audio ?? (file?.audio.find((a) => a.default) ?? file?.audio[0])?.index ?? -1);
  const [sub, setSub] = createSignal(prefs.sub ?? -1);

  let video!: HTMLVideoElement;
  let root!: HTMLDivElement;
  let playBtn!: HTMLButtonElement;
  let bar!: HTMLDivElement;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let adapter: HlsJsAdapter | undefined;

  const poke = () => {
    setVisible(true);
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (playing() && !picker() && scrub() == null) setVisible(false);
    }, HIDE_AFTER);
  };

  // Episode → show title for the top bar (two hops: episode → season → show).
  if (props.item.kind === "episode" && props.item.parent_id != null) {
    api.item(props.item.parent_id)
      .then((season) => (season.parent_id != null ? api.item(season.parent_id) : season))
      .then((show) => setSeries(show.title), () => undefined);
  }

  const startAt = () => {
    if (search.start === "0") return 0;
    if (search.t) return Number(search.t) || 0;
    const p = props.item.progress;
    return p && p < 0.95 && file ? p * file.duration_s : 0;
  };

  const toggle = () => {
    if (!adapter) return;
    if (video.paused) adapter.play().catch(() => undefined);
    else adapter.pause();
    poke();
  };
  const seekBy = (delta: number) => {
    const base = scrub() ?? pos();
    const d = dur() || file?.duration_s || 0;
    const target = Math.max(0, Math.min(d - 1, base + delta));
    setScrub(target);
    poke();
    commitLater();
  };
  let commitTimer: ReturnType<typeof setTimeout> | undefined;
  const commitLater = () => {
    if (commitTimer) clearTimeout(commitTimer);
    commitTimer = setTimeout(() => {
      const s = scrub();
      if (s != null && adapter) {
        adapter.seek(s);
        setPos(s);
      }
      setScrub(null);
    }, 450);
  };

  onMount(() => {
    if (!file) {
      setError("No playable file for this title.");
      return;
    }
    adapter = new HlsJsAdapter(video);
    const a = adapter;
    a.on("timeupdate", (t) => {
      setPos(t.positionSec);
      if (t.durationSec && isFinite(t.durationSec)) setDur(t.durationSec);
    });
    const start = startAt();
    a.on("ready", (r) => {
      if (r.durationSec && isFinite(r.durationSec)) setDur(r.durationSec);
      // Resume point beyond the actual media (stale progress, replaced file) → start over.
      if (start > 0 && r.durationSec && start < r.durationSec - 10) a.seek(start);
    });
    a.on("buffered", (b) => setBuffered(b.endSec));
    a.on("play", () => {
      setPlaying(true);
      poke();
    });
    a.on("pause", () => {
      setPlaying(false);
      setVisible(true);
    });
    a.on("buffering", (b) => setBuffering(b));
    a.on("ended", () => {
      setPlaying(false);
      setVisible(true);
    });
    a.on("error", (e) => {
      if (e.fatal) setError(e.message || "Playback failed");
    });
    useProgressReporter({ itemId: props.item.id, fileId: file.id, adapter: a });

    a.load(mediaUrl(file.url)!, { autoplay: true })
      .catch(() => {
        // Autoplay blocked: show the play button instead.
        setPlaying(false);
        setBuffering(false);
      });
    video.addEventListener("canplay", () => setBuffering(false));

    focusElement(playBtn, { smooth: false });
    poke();

    // Remote / keyboard. Window-capture runs before the spatial navigator.
    const onKey = (e: KeyboardEvent) => {
      if (picker()) return;
      const k = e.key;
      // Back is handled by onBack below; don't let it re-show the controls first.
      if (k === "Escape" || k === "Backspace" || k === "GoBack" || e.keyCode === 461 || e.keyCode === 10009) return;
      const hidden = !visible();
      if (k === " " || k === "k") {
        e.preventDefault();
        toggle();
        return;
      }
      if (k === "f" && !e.ctrlKey && !e.metaKey) {
        fullscreen();
        return;
      }
      if (hidden && (k === "ArrowLeft" || k === "ArrowRight")) {
        // Seek straight away from the "clean" screen, like a TV remote expects.
        e.preventDefault();
        e.stopImmediatePropagation();
        seekBy(k === "ArrowLeft" ? -10 : 10);
        focusElement(bar, { smooth: false });
        return;
      }
      if (hidden && /^(ArrowUp|ArrowDown|Enter)$/.test(k)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        poke();
        return;
      }
      poke();
    };
    window.addEventListener("keydown", onKey, true);
    const offMedia = onMediaKey((m) => {
      poke();
      if (!adapter) return;
      if (m === "playpause") toggle();
      else if (m === "play") adapter.play().catch(() => undefined);
      else if (m === "pause") adapter.pause();
      else if (m === "ff") seekBy(30);
      else if (m === "rw") seekBy(-10);
      else if (m === "stop") exit();
    });
    const offBack = onBack(() => {
      if (picker()) return false; // Sheet handles it
      if (visible() && playing()) {
        // First Back hides the controls, second one leaves.
        if (hideTimer) clearTimeout(hideTimer);
        setVisible(false);
        return true;
      }
      exit();
      return true;
    });

    onCleanup(() => {
      window.removeEventListener("keydown", onKey, true);
      offMedia();
      offBack();
      if (hideTimer) clearTimeout(hideTimer);
      if (commitTimer) clearTimeout(commitTimer);
      adapter?.destroy();
      if (document.fullscreenElement) document.exitFullscreen().catch(() => undefined);
    });
  });

  const exit = () => {
    if (window.history.length > 1) window.history.back();
    else navigate(`/item/${props.item.id}`);
  };
  const fullscreen = () => {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => undefined);
    else if (root.requestFullscreen) root.requestFullscreen().catch(() => undefined);
  };

  // Scrubber: arrows seek with acceleration on hold.
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
    const r = bar.getBoundingClientRect();
    const p = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
    setScrub(p * (dur() || 0));
    if (e.type === "pointerup" || e.type === "click") commitLater();
  };

  const shown = () => scrub() ?? pos();
  const pct = () => (dur() ? (shown() / dur()) * 100 : 0);
  const bufPct = () => (dur() ? Math.min(100, (buffered() / dur()) * 100) : 0);
  const audioText = () => {
    const a = file?.audio.find((x) => x.index === audio());
    return a ? `${audioLabel(a).title} ${channelsLabel(a.channels)}` : "Default";
  };
  const subText = () => {
    const s = file?.subtitles.find((x) => x.index === sub());
    return s ? subtitleLabel(s).title : "Off";
  };

  return (
    <div
      ref={root}
      class="player__stage"
      classList={{ "is-chrome": visible() || !playing(), "is-paused": !playing() }}
      onMouseMove={poke}
    >
      <video
        ref={video}
        class="player__video"
        playsinline
        preload="auto"
        onClick={toggle}
        onDblClick={fullscreen}
      />

      <Show when={buffering() && !error()}>
        <div class="player__buffer" aria-label="Buffering"><span class="spinner" /></div>
      </Show>

      <Show when={error()}>
        <div class="player__error" role="alert">
          <Icon name="alert" />
          <h2>Can't play this file</h2>
          <p>{error()} This browser may not support the file's format — the transcoding engine will handle this in a later release.</p>
          <button type="button" class="btn btn--primary" data-autofocus onClick={exit}>
            <Icon name="back" /> <span>Go back</span>
          </button>
        </div>
      </Show>

      <div class="player__chrome" data-nav-fixed aria-hidden={!visible() && playing() ? "true" : undefined}>
        <div class="player__top">
          <button type="button" class="iconbtn" aria-label="Back" onClick={exit}>
            <Icon name="back" />
          </button>
          <div class="player__titles">
            <Show when={props.item.kind === "episode"} fallback={<h1>{props.item.title}</h1>}>
              <p class="eyebrow">{series() ?? ""}{series() ? " · " : ""}{episodeCode(props.item)}</p>
              <h1>{props.item.title}</h1>
            </Show>
          </div>
          <span class="player__mode">Direct play</span>
        </div>

        <div class="player__bottom">
          <div class="scrub">
            <span class="scrub__time">{formatClock(shown())}</span>
            <div
              ref={bar}
              class="scrub__bar"
              role="slider"
              tabindex="0"
              data-focusable
              data-nav-capture="left right"
              aria-label="Seek"
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
                <i class="scrub__buf" style={{ width: `${bufPct()}%` }} />
                <i class="scrub__fill" style={{ width: `${pct()}%` }} />
              </div>
              <span class="scrub__thumb" style={{ left: `${pct()}%` }} />
              <Show when={scrub() != null}>
                <span class="scrub__bubble" style={{ left: `${pct()}%` }}>{formatClock(scrub()!)}</span>
              </Show>
            </div>
            <span class="scrub__time scrub__time--end">-{formatClock(Math.max(0, dur() - shown()))}</span>
          </div>

          <div class="player__controls">
            <div class="player__cluster">
              <button type="button" class="iconbtn" aria-label="Back 10 seconds" onClick={() => seekBy(-10)}>
                <Icon name="replay" />
                <small>10</small>
              </button>
              <button type="button" class="iconbtn iconbtn--xl" ref={playBtn} aria-label={playing() ? "Pause" : "Play"} onClick={toggle}>
                <Icon name={playing() ? "pause" : "play"} />
              </button>
              <button type="button" class="iconbtn" aria-label="Forward 30 seconds" onClick={() => seekBy(30)}>
                <Icon name="skip" />
                <small>30</small>
              </button>
            </div>
            <div class="player__opts">
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("audio")} disabled={!file?.audio.length}>
                <Icon name="audio" /> <span class="btn__value">{audioText()}</span>
              </button>
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("sub")}>
                <Icon name="cc" /> <span class="btn__value">{subText()}</span>
              </button>
              <button type="button" class="iconbtn player__fs" aria-label="Full screen" onClick={fullscreen}>
                <Icon name="fullscreen" />
              </button>
            </div>
          </div>
        </div>
      </div>

      <Show when={picker() === "audio" && file}>
        <Sheet
          title="Audio"
          selected={audio()}
          options={file!.audio.map((a) => ({ value: a.index, ...audioLabel(a) }))}
          onSelect={(v) => {
            setAudio(v);
            setTrackPrefs(props.item.id, { audio: v });
            // STUB: switching tracks mid-stream needs the playback engine (Phase 2, /playback/{fileId}/decide).
          }}
          onClose={() => {
            setPicker(null);
            poke();
          }}
        />
      </Show>
      <Show when={picker() === "sub" && file}>
        <Sheet
          title="Subtitles"
          selected={sub()}
          options={[{ value: -1, title: "Off" } as { value: number; title: string; detail?: string }].concat(
            file!.subtitles.map((s) => ({ value: s.index, ...subtitleLabel(s) })),
          )}
          onSelect={(v) => {
            setSub(v);
            setTrackPrefs(props.item.id, { sub: v });
            // STUB: text subs will load from /subs/{streamId}.vtt once the API exposes stream ids.
          }}
          onClose={() => {
            setPicker(null);
            poke();
          }}
        />
      </Show>
    </div>
  );
}


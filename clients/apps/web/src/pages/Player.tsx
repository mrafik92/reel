import { createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import {
  Icon, Sheet, audioLabel, channelsLabel, createPlayerAdapter, episodeCode, focusElement, formatClock,
  AvPlayAdapter, languageName, mediaUrl, onBack, onRemoteKey,
  type ItemDetail, type MediaFile, type PlayerAdapter, type PlayerTracks,
} from "@reel/ui";
import { api, ApiError, invalidate, type DecideResponse, type SubtitleOption } from "../api";
import { creditItem, decide, getTrackPrefs, MODE_LABEL, setTrackPrefs } from "../lib/playback";
import { ErrorState } from "../components/States";
import { getServerUrl } from "@reel/ui";

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
const TICK_MS = 10_000;

interface PlayError {
  title: string;
  body: string;
  retry: boolean;
}

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
  const [error, setError] = createSignal<PlayError>();
  const [scrub, setScrub] = createSignal<number | null>(null);
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [series, setSeries] = createSignal<string>();
  const [session, setSession] = createSignal<DecideResponse>();
  const [info, setInfo] = createSignal(false);
  const [cue, setCue] = createSignal<string | null>(null);
  /** Track list reported by native adapters (webOS <video>, Tizen AVPlay). */
  const [nativeTracks, setNativeTracks] = createSignal<PlayerTracks>();
  /** AVPlay draws on a plane *behind* the page: keep everything above it transparent. */
  const [nativePlane, setNativePlane] = createSignal(false);

  const prefs = getTrackPrefs(props.item.id);
  const [audio, setAudio] = createSignal<number | null>(prefs.audio ?? null);
  const [sub, setSub] = createSignal(prefs.sub ?? -1);

  let video!: HTMLVideoElement;
  let root!: HTMLDivElement;
  let playBtn!: HTMLButtonElement;
  let bar!: HTMLDivElement;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let tickTimer: ReturnType<typeof setInterval> | undefined;
  let adapter: PlayerAdapter | undefined;
  let offs: (() => void)[] = [];
  let closed = false;

  const poke = () => {
    setVisible(true);
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (playing() && !picker() && scrub() == null && !info()) setVisible(false);
    }, HIDE_AFTER);
  };

  // Episode → show title for the top bar (two hops: episode → season → show).
  if (props.item.kind === "episode" && props.item.parent_id != null) {
    api.item(props.item.parent_id)
      .then((season) => (season.parent_id != null ? api.item(season.parent_id) : season))
      .then((show) => setSeries(show.title), () => undefined);
  }

  const resumeAt = () => {
    if (search.start === "0") return 0;
    if (search.t) return Number(search.t) || 0;
    const p = props.item.progress;
    return p && p < 0.95 && file ? p * file.duration_s : 0;
  };

  // ---- session lifecycle -------------------------------------------------
  const report = (paused: boolean, keepalive = false) => {
    const s = session();
    if (!s || pos() < 1) return;
    api.progress(s.session_id, { position_s: pos(), paused }, keepalive).catch(() => undefined);
  };
  const endSession = (keepalive = false) => {
    const s = session();
    if (!s) return;
    report(true, keepalive);
    api.stopPlayback(s.session_id, keepalive).catch(() => undefined);
    setSession(undefined);
  };

  /** (Re)start playback: decide → load. Used on open and when audio/burn-in subs change. */
  const start = async (at: number) => {
    if (!file) return;
    setError(undefined);
    setBuffering(true);
    endSession();
    let d: DecideResponse;
    try {
      d = await decide(file.id, { itemId: creditItem(file, props.item.id), audio: audio(), subtitle: sub(), start: at });
    } catch (e) {
      setBuffering(false);
      setError(decideError(e));
      return;
    }
    if (closed) {
      api.stopPlayback(d.session_id).catch(() => undefined);
      return;
    }
    setSession(d);
    if (audio() == null) setAudio(d.decision.audio_index ?? d.audio.find((a) => a.default)?.index ?? d.audio[0]?.index ?? null);
    offs.forEach((f) => f());
    offs = [];
    adapter?.destroy();
    clearTracks();
    const a = createPlayerAdapter(video);
    adapter = a;
    setNativePlane(a instanceof AvPlayAdapter);
    setNativeTracks(undefined);
    wire(a, at);
    const textTracks = d.subtitles
      .filter((s) => s.url)
      .map((s) => ({ id: String(s.index), url: mediaUrl(s.url)!, label: subName(s), language: s.language ?? undefined }));
    try {
      await a.load(mediaUrl(d.url)!, { startPositionSec: at || undefined, autoplay: true, textTracks });
    } catch {
      // Autoplay blocked: show the play button instead.
      setPlaying(false);
      setBuffering(false);
    }
    attachTracks(a, d.subtitles);
    if (a.getTracks) setNativeTracks(a.getTracks());
    applySubtitle(d);
  };

  function wire(a: PlayerAdapter, startAt: number): void {
    let first = true;
    offs.push(
      a.on("timeupdate", (t) => {
        // Stale resume point past the real end (file replaced, bad progress) → start over.
        if (first && t.durationSec && isFinite(t.durationSec)) {
          first = false;
          if (startAt > 0 && startAt >= t.durationSec - 5) {
            a.seek(0);
            a.play().catch(() => undefined);
          }
        }
        setPos(t.positionSec);
        if (t.durationSec && isFinite(t.durationSec)) setDur(t.durationSec);
      }),
      a.on("ready", (r) => r.durationSec && isFinite(r.durationSec) && setDur(r.durationSec)),
      a.on("buffered", (b) => setBuffered(b.endSec)),
      a.on("play", () => {
        setPlaying(true);
        setBuffering(false);
        poke();
      }),
      a.on("pause", () => {
        setPlaying(false);
        setVisible(true);
        report(true);
      }),
      a.on("buffering", (b) => setBuffering(b)),
      a.on("ended", () => {
        setPlaying(false);
        setVisible(true);
        report(true);
      }),
      // Tizen AVPlay can't show <track>s: the adapter emits cues and we draw them.
      a.on("cue", (c) => setCue(c.text)),
      a.on("tracks", (t) => setNativeTracks(t)),
      a.on("error", (e) => {
        if (e.fatal) {
          setBuffering(false);
          setError({
            title: "Playback stopped",
            body: `${e.message || "The stream failed"}. Try again, or pick a different version of this title.`,
            retry: true,
          });
        }
      }),
    );
  }

  // ---- subtitles -----------------------------------------------------------
  /** Web (hls.js / direct): sidecar VTT as <track> children. Native adapters take `textTracks`. */
  function attachTracks(a: PlayerAdapter, subs: SubtitleOption[]): void {
    if (a.selectTextTrack) return;
    for (const s of subs) {
      if (!s.url) continue;
      const t = document.createElement("track");
      t.kind = "subtitles";
      t.label = subName(s);
      t.srclang = s.language ?? "";
      t.src = mediaUrl(s.url)!;
      t.setAttribute("data-index", String(s.index));
      video.appendChild(t);
    }
  }
  function clearTracks(): void {
    Array.prototype.slice.call(video.querySelectorAll("track")).forEach((t: Element) => t.remove());
    setCue(null);
  }
  function applySubtitle(d: DecideResponse | undefined = session()): void {
    const want = sub();
    // Burned-in subs are part of the picture; nothing to toggle client-side.
    if (d?.decision.subtitle.type === "burn_in") return;
    if (adapter?.selectTextTrack) {
      adapter.selectTextTrack(want >= 0 ? String(want) : null);
      return;
    }
    const tracks = video.textTracks;
    const els = video.querySelectorAll("track");
    for (let i = 0; i < tracks.length; i++) {
      const idx = els[i]?.getAttribute("data-index");
      tracks[i]!.mode = idx != null && Number(idx) === want ? "showing" : "disabled";
    }
  }
  const chooseSub = (v: number) => {
    setSub(v);
    setTrackPrefs(props.item.id, { sub: v });
    const s = session();
    const opt = s?.subtitles.find((x) => x.index === v);
    const burned = s?.decision.subtitle.type === "burn_in";
    // Image subs (no VTT url) need a burn-in transcode; leaving burn-in needs a clean stream.
    if ((v >= 0 && opt && !opt.url) || burned) void start(pos());
    else applySubtitle();
  };
  const chooseAudio = (v: number) => {
    if (v === audio()) return;
    setAudio(v);
    setTrackPrefs(props.item.id, { audio: v });
    // Native players can switch tracks inside the file they're already playing
    // (direct play / remux copy all audio). Server streams are listed in the same
    // order, so match by position; otherwise ask the server for a new stream.
    const s = session();
    const t = nativeTracks()?.audio;
    const pos0 = s ? s.audio.findIndex((a) => a.index === v) : -1;
    if (adapter?.selectAudioTrack && s && s.decision.audio.type === "copy" && t && t.length === s.audio.length && pos0 >= 0) {
      adapter.selectAudioTrack(t[pos0]!.id);
      return;
    }
    void start(pos());
  };

  // ---- transport -------------------------------------------------------------
  const toggle = () => {
    if (!adapter) return;
    // Use our state, not video.paused: AVPlay doesn't drive the <video> element.
    if (!playing()) adapter.play().catch(() => undefined);
    else adapter.pause();
    poke();
  };
  let commitTimer: ReturnType<typeof setTimeout> | undefined;
  const seekBy = (delta: number) => {
    const base = scrub() ?? pos();
    const d = dur() || file?.duration_s || 0;
    setScrub(Math.max(0, Math.min(d - 1, base + delta)));
    poke();
    commitLater();
  };
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

  const exit = () => {
    if (window.history.length > 1) window.history.back();
    else navigate(`/item/${props.item.id}`);
  };
  const fullscreen = () => {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => undefined);
    else if (root.requestFullscreen) root.requestFullscreen().catch(() => undefined);
  };

  onMount(() => {
    if (!file) {
      setError({ title: "Nothing to play", body: "This title has no playable file yet.", retry: false });
      setBuffering(false);
      return;
    }
    void start(resumeAt());
    focusElement(playBtn, { smooth: false });
    poke();

    tickTimer = setInterval(() => {
      if (playing()) report(false);
    }, TICK_MS);

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
      if (k === "i" && !e.ctrlKey && !e.metaKey) {
        setInfo((v) => !v);
        poke();
        return;
      }
      if (k === "f" && !e.ctrlKey && !e.metaKey) {
        fullscreen();
        return;
      }
      if (hidden && (k === "ArrowLeft" || k === "ArrowRight")) {
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
    // Remote transport keys (normalised by the platform layer). Digits and
    // channel up/down stay unhandled here: reserved for Live TV.
    const offMedia = onRemoteKey((e) => {
      const m = e.key;
      if (m !== "play" && m !== "pause" && m !== "playpause" && m !== "ff" && m !== "rw" && m !== "stop" && m !== "info") return false;
      poke();
      if (!adapter) return true;
      if (m === "playpause") toggle();
      else if (m === "play") adapter.play().catch(() => undefined);
      else if (m === "pause") adapter.pause();
      else if (m === "ff") seekBy(30);
      else if (m === "rw") seekBy(-10);
      else if (m === "stop") exit();
      else if (m === "info") setInfo((v) => !v);
      return true;
    });
    const offBack = onBack(() => {
      if (picker()) return false;
      if (info()) {
        setInfo(false);
        return true;
      }
      if (visible() && playing()) {
        if (hideTimer) clearTimeout(hideTimer);
        setVisible(false);
        return true;
      }
      exit();
      return true;
    });
    const onUnload = () => endSession(true);
    window.addEventListener("pagehide", onUnload);

    onCleanup(() => {
      closed = true;
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pagehide", onUnload);
      offMedia();
      offBack();
      if (hideTimer) clearTimeout(hideTimer);
      if (commitTimer) clearTimeout(commitTimer);
      if (tickTimer) clearInterval(tickTimer);
      endSession(true);
      offs.forEach((f) => f());
      adapter?.destroy();
      invalidate("home");
      invalidate("item");
      if (document.fullscreenElement) document.exitFullscreen().catch(() => undefined);
    });
  });

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
    if (e.type === "pointerup") commitLater();
  };

  const shown = () => scrub() ?? pos();
  const pct = () => (dur() ? (shown() / dur()) * 100 : 0);
  const bufPct = () => (dur() ? Math.min(100, (buffered() / dur()) * 100) : 0);
  const audioList = () => session()?.audio ?? file?.audio ?? [];
  const subList = () => session()?.subtitles ?? [];
  const audioText = () => {
    const a = audioList().find((x) => x.index === audio());
    return a ? `${audioLabel(a).title} ${channelsLabel(a.channels)}` : "Default";
  };
  const subText = () => {
    const s = subList().find((x) => x.index === sub());
    return s ? subName(s) : "Off";
  };
  const mode = () => session()?.mode;

  return (
    <div
      ref={root}
      class="player__stage"
      classList={{ "is-chrome": visible() || !playing(), "is-paused": !playing(), "is-native-plane": nativePlane() }}
      onMouseMove={poke}
    >
      <video
        ref={video}
        class="player__video"
        playsinline
        preload="auto"
        crossorigin={getServerUrl() ? "anonymous" : undefined}
        onClick={toggle}
        onDblClick={fullscreen}
      />

      <Show when={cue()}>
        <div class="player__cue" aria-live="polite">{cue()}</div>
      </Show>

      <Show when={buffering() && !error()}>
        <div class="player__buffer" aria-label="Buffering"><span class="spinner" /></div>
      </Show>

      <Show when={error()}>
        {(e) => (
          <div class="player__error" role="alert">
            <Icon name="alert" />
            <h2>{e().title}</h2>
            <p>{e().body}</p>
            <div class="actions actions--center">
              <Show when={e().retry}>
                <button type="button" class="btn btn--primary" data-autofocus onClick={() => void start(pos() || resumeAt())}>
                  <Icon name="retry" /> <span>Try again</span>
                </button>
              </Show>
              <button type="button" class="btn" data-autofocus={e().retry ? undefined : ""} onClick={exit}>
                <Icon name="back" /> <span>Go back</span>
              </button>
            </div>
          </div>
        )}
      </Show>

      <Show when={info() && session()}>
        {(s) => <StreamInfo session={s()} file={file} onClose={() => setInfo(false)} />}
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
          <Show when={mode()}>
            <button
              type="button"
              class="player__mode"
              classList={{ "is-transcode": mode() === "transcode" || mode() === "audio_transcode" }}
              aria-pressed={info()}
              onClick={() => setInfo((v) => !v)}
            >
              {MODE_LABEL[mode()!] ?? mode()}
            </button>
          </Show>
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
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("audio")} disabled={audioList().length < 2}>
                <Icon name="audio" /> <span class="btn__value">{audioText()}</span>
              </button>
              <button type="button" class="btn btn--ghost btn--select" onClick={() => setPicker("sub")} disabled={!subList().length}>
                <Icon name="cc" /> <span class="btn__value">{subText()}</span>
              </button>
              <button type="button" class="iconbtn" aria-label="Stream info" aria-pressed={info()} onClick={() => setInfo((v) => !v)}>
                <Icon name="info" />
              </button>
              <button type="button" class="iconbtn player__fs" aria-label="Full screen" onClick={fullscreen}>
                <Icon name="fullscreen" />
              </button>
            </div>
          </div>
        </div>
      </div>

      <Show when={picker() === "audio"}>
        <Sheet
          title="Audio"
          selected={audio() ?? -1}
          options={audioList().map((a) => ({ value: a.index, ...audioLabel(a) }))}
          onSelect={chooseAudio}
          onClose={() => {
            setPicker(null);
            poke();
          }}
        />
      </Show>
      <Show when={picker() === "sub"}>
        <Sheet
          title="Subtitles"
          selected={sub()}
          options={[{ value: -1, title: "Off" } as { value: number; title: string; detail?: string }].concat(
            subList().map((s) => ({
              value: s.index,
              title: subName(s),
              detail: [s.forced ? "Forced" : "", s.external ? "External" : "", s.url ? "" : "Burned in (transcodes)"]
                .filter(Boolean)
                .join(" · ") || undefined,
            })),
          )}
          onSelect={chooseSub}
          onClose={() => {
            setPicker(null);
            poke();
          }}
        />
      </Show>
    </div>
  );
}

function subName(s: { title?: string | null; language?: string | null }): string {
  return s.title || languageName(s.language);
}

function decideError(e: unknown): PlayError {
  if (e instanceof ApiError) {
    if (e.status === 429) {
      return {
        title: "Stream limit reached",
        body: `${e.message}. Stop playback on another device${e.retryAfter ? `, or try again in ${Math.ceil(e.retryAfter / 60)} min` : ""}.`,
        retry: true,
      };
    }
    if (e.status === 404) return { title: "File not found", body: "This file is no longer on the server. A rescan may fix it.", retry: false };
    return { title: "Can't start playback", body: e.message, retry: true };
  }
  return { title: "Can't start playback", body: "Something went wrong talking to the server.", retry: true };
}

function actionText(a: { type: string; codec?: string; channels?: number; width?: number; height?: number; bitrate_kbps?: number; tonemap?: boolean; format?: string }): string {
  switch (a.type) {
    case "copy": return "Copy (no re-encode)";
    case "none": return "None";
    case "encode":
      return [a.codec?.toUpperCase(), a.width && a.height ? `${a.width}×${a.height}` : "", a.channels ? `${a.channels}ch` : "",
        a.bitrate_kbps ? `${a.bitrate_kbps} kbps` : "", a.tonemap ? "tone-mapped" : ""].filter(Boolean).join(" · ");
    case "sidecar": return `Sidecar (${a.format ?? "vtt"})`;
    case "burn_in": return "Burned into video";
    case "native": return "Rendered by player";
    default: return a.type;
  }
}

/** Debug overlay: how the server decided to deliver this stream, and why. */
function StreamInfo(props: { session: DecideResponse; file?: MediaFile; onClose: () => void }): JSX.Element {
  const d = () => props.session.decision;
  return (
    <aside class="stream-info" aria-label="Stream info">
      <header>
        <p class="eyebrow eyebrow--accent">Stream info</p>
        <strong>{MODE_LABEL[props.session.mode] ?? props.session.mode}</strong>
      </header>
      <dl>
        <dt>Video</dt>
        <dd>{actionText(d().video as never)}</dd>
        <dt>Audio</dt>
        <dd>{actionText(d().audio as never)}</dd>
        <dt>Subtitles</dt>
        <dd>{actionText(d().subtitle as never)}</dd>
        <Show when={props.file}>
          <dt>Source</dt>
          <dd>{props.file!.container.toUpperCase()}{props.file!.video ? ` · ${props.file!.video.codec.toUpperCase()} ${props.file!.video.width}×${props.file!.video.height}` : ""}</dd>
        </Show>
        <dt>Session</dt>
        <dd class="mono">{props.session.session_id.slice(0, 12)}</dd>
      </dl>
      <Show when={d().reasons.length}>
        <p class="stream-info__label">Why</p>
        <ul>
          <For each={d().reasons}>{(r) => <li>{r}</li>}</For>
        </ul>
      </Show>
      <p class="stream-info__hint"><span class="keycap">i</span> or <span class="keycap">Back</span> to close</p>
    </aside>
  );
}

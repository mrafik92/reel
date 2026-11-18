import { startupTiming, type StartupTiming } from "../lib/startupTiming";
import { createEffect, createResource, createSignal, For, onCleanup, onMount, Show, useContext, type JSX } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import {
  Icon, Sheet, audioLabel, codecShort, createPlayerAdapter, episodeCode, focusElement, iso1, isSdh,
  AvPlayAdapter, mediaUrl, onBack, onRemoteKey, playsHlsWithHlsJs, subtitleLabel,
  type IconName, type ItemDetail, type MediaFile, type PlayerAdapter, type PlayerTracks,
} from "@reel/ui";
import { api, ApiError, invalidate, type DecideResponse, type Quality, type SubtitleOption, type VariantOption } from "../api";
import { casting, type CastPlayBody } from "../api/casting";
import { selectedOf, type Selected } from "../api/prefs";
import { creditItem, decide, MODE_LABEL } from "../lib/playback";
import {
  QUALITIES, QUALITY_NAME, deviceQuality, explainStream, fmtMbps, optionDetail, qualityOptions, userLimitKbps, videoTranscoded,
  whyTranscode,
} from "../lib/quality";
import {
  StallCounter, bandwidthEstimate, capEstimate, isAdaptive, probeDue, probeThroughput, recordThroughput, variantOf, variantSelect,
} from "../lib/abr";
import { audioQuickSave, previewSelection, subtitleQuickSave, type QuickSave } from "../lib/tracks";
import { nativeAudioChoice } from "../lib/nativeAudio";
import { loadPrefs, playbackPrefs, prefsSupported, updatePrefs } from "../lib/prefs";
import {
  castFor, onCastEnd, startCast, stopCast, type CastOpen, type CastSource, type CastStream, type CastTarget, type CastTrack,
} from "../lib/casting";
import { CastCancelled } from "../lib/casting/chromecast";
import { TV, href } from "../lib/mode";
import { toast } from "../lib/toast";
import { ErrorState } from "../components/States";
import { CtlButton, Osd, Scrubber, Transport, Volume, useOsd, type ScrubberApi } from "../components/player/Controls";
import { playHref, toPath } from "../lib/links";
import { createDoubleTap, createTapToggle, pointerKind, zoneOf, type TapResult } from "../lib/tap";
import { CastSheet } from "../components/casting/CastSheet";
import { CastRemote } from "../components/casting/CastRemote";
import { getServerUrl } from "@reel/ui";
import { FullscreenHost, leaveFullscreen, playerFullscreen } from "../lib/fullscreen";

export default function Player(): JSX.Element {
  const params = useParams<{ id: string }>();
  let timing = startupTiming("library");
  onCleanup(() => timing?.finish("canceled"));
  const [item, { refetch }] = createResource(() => Number(params.id), (id) => api.item(id));
  createEffect(() => { if (item.error) timing?.finish("failed"); });
  // Full screen lives on this wrapper, which outlives the episode swap below (#247).
  let host: HTMLDivElement | undefined;
  onCleanup(() => leaveFullscreen(document));
  return (
    <div class="player" ref={host}>
      <FullscreenHost.Provider value={() => host}>
        <Show when={!item.error} fallback={<ErrorState error={item.error} onRetry={() => { timing = startupTiming("library"); void refetch(); }} />}>
          {/* keyed: "Next episode" swaps the item under the same route → fresh player */}
          <Show when={item()} keyed fallback={<div class="player__loading"><span class="spinner" /></div>}>
            {(it) => <PlayerView item={it} startup={timing} />}
          </Show>
        </Show>
      </FullscreenHost.Provider>
    </div>
  );
}

/** Controls fade after this long without input (a little longer on TV: remotes are slower). */
const HIDE_AFTER = TV ? 5000 : 3500;
const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];
const VOL_KEY = "reel.player.volume";
const TICK_MS = 10_000;
/**
 * A paused player keeps its session alive with "paused" pings for this long (#95); the server then
 * keeps a converted session for its paused timeout (30 min). Past that the transcode slot is freed,
 * and pressing play opens a new session at the same spot.
 */
const PAUSED_KEEPALIVE_MS = 30 * 60_000;
/** Paused at least this long: check the session is still there as soon as playback resumes. */
const RESUME_CHECK_MS = 45_000;

export interface PlayError {
  title: string;
  body: string;
  retry: boolean;
  /** Extra fix-it action (e.g. "Stop the other stream"); playback restarts after it resolves. */
  action?: { label: string; run: () => Promise<void>; icon?: IconName; busy?: string };
}

/**
 * Non-library playback (IPTV on-demand): the page hands in a synthetic item (title, runtime,
 * progress) and how to open a session; everything else — tracks, subtitles, progress pings,
 * stop — is the regular player.
 */
export interface PlaySource {
  /**
   * Open a session. `subtitle`: -1 = off / not chosen (legacy). Newer fields — optional to
   * honour, older sources keep working:
   *  - `subtitleIndex`: wire value for `subtitle_index` (null = automatic from the user's
   *    preferences, -1 = off, n = that stream),
   *  - `forCast`: the stream is for a Chromecast / AirPlay receiver → send `for_cast: true` and
   *    `preset: forCast` (absolute, long-lived URLs). If the source ignores it, the player
   *    converts the session with `POST /playback/{id}/cast-url`.
   */
  decide: (o: {
    audio: number | null;
    subtitle: number;
    start: number;
    subtitleIndex?: number | null;
    forCast?: "chromecast" | "airplay";
    /** Playback quality (#87): send as `quality`. Omitted for cast receivers. */
    quality?: Quality;
  }) => Promise<DecideResponse>;
  /** DLNA "Play on TV": what the server should push (omit → DLNA TVs aren't offered). */
  dlna?: (o: { audio: number | null; subtitle: number | null; start: number }) => CastPlayBody;
  /** Map a start failure to a friendly error (falls back to the library wording). */
  error?: (e: unknown) => PlayError | undefined;
  /** Eyebrow above the title (e.g. series · S1 E3). */
  eyebrow?: string;
  /** Where Back goes when there's no history. */
  back: string;
  onClose?: () => void;
}

export function PlayerView(props: { item: ItemDetail; source?: PlaySource; startup?: StartupTiming }): JSX.Element {
  const navigate = useNavigate();
  const [search, setSearch] = useSearchParams<{ file?: string; start?: string; t?: string; audio?: string; sub?: string; cast?: string }>();
  const file: MediaFile | undefined =
    props.item.files.find((f) => String(f.id) === search.file) ?? props.item.files[0];

  /** Human label for a subtitle (session options lack the codec, so merge it from the file). */
  const subLabel = (s: { index: number; language?: string | null; title?: string | null; forced: boolean; external: boolean }) =>
    subtitleLabel({ ...s, codec: file?.subtitles.find((x) => x.index === s.index)?.codec });

  const [playing, setPlaying] = createSignal(false);
  const [buffering, setBuffering] = createSignal(true);
  const [pos, setPos] = createSignal(0);
  const [dur, setDur] = createSignal(file?.duration_s ?? 0);
  const [buffered, setBuffered] = createSignal(0);
  const [visible, setVisible] = createSignal(true);
  const [error, setError] = createSignal<PlayError>();
  /** The player is getting the stream back by itself (#91): what it's doing, for the overlay. */
  const [recovering, setRecovering] = createSignal<string>();
  /** Last automatic session restart after the player gave up (at most one per minute). */
  let autoRestartAt = 0;
  /**
   * The viewer paused (#100). Automatic restarts (a reopened session, a track change) keep the
   * player paused instead of starting it; only the viewer's play does that.
   */
  let held = false;
  /** When the player was paused (0 while playing): paused pings stop after PAUSED_KEEPALIVE_MS. */
  let pausedAt = 0;
  /** The server no longer knows this session (reaped, server restarted): reopen on the next play. */
  let gone = false;
  const [scrub, setScrub] = createSignal<number | null>(null);
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [series, setSeries] = createSignal<string>();
  const [session, setSession] = createSignal<DecideResponse>();
  const [info, setInfo] = createSignal(false);
  /**
   * Quality this play asks for (#87): the device default (Settings → Playback), until the viewer
   * picks another in the player. Switching re-decides from the playhead with the same device id,
   * so the server replaces the old session.
   */
  const [quality, setQuality] = createSignal<Quality>(deviceQuality());
  const [qualitySheet, setQualitySheet] = createSignal(false);
  /** The server offers quality choices (sticky, so the button keeps focus while a switch reopens the session). */
  const [qualityOk, setQualityOk] = createSignal(false);
  /**
   * Multi-variant sessions (#111): the variant hls.js is playing. Quality choices switch it locally
   * (pin / cap); only a choice the master can't meet re-decides, as a single-variant session at that
   * quality (`singleFor`), which is what #87 did.
   */
  const [variantId, setVariantId] = createSignal<string>();
  let singleFor: Quality | undefined;
  /** Direct play under Auto: stalls that switch to an adaptive session (#1), once per play. */
  const stalls = new StallCounter();
  let directFallback = false;
  let probed = false;
  /** Sanitised HTML of the active subtitle cue(s); we render all subtitles ourselves. */
  const [cueHtml, setCueHtml] = createSignal("");
  /** Px from the bottom that clears the control bar while it's visible. */
  const [subLift, setSubLift] = createSignal(0);
  /** Track list reported by native adapters (webOS <video>, Tizen AVPlay). */
  const [nativeTracks, setNativeTracks] = createSignal<PlayerTracks>();
  /** AVPlay draws on a plane *behind* the page: keep everything above it transparent. */
  const [nativePlane, setNativePlane] = createSignal(false);

  // Tracks: picked on the detail page (URL) → else the server's automatic choice from the
  // user's preferences (`selected`). A pick in the player overrides it for this session.
  const urlNum = (v?: string) => (v != null && v !== "" && isFinite(Number(v)) ? Number(v) : null);
  const [audio, setAudio] = createSignal<number | null>(urlNum(search.audio));
  const [sub, setSub] = createSignal(urlNum(search.sub) ?? -1);
  /** false until the first session: ask the server to choose (`subtitle_index: null`). */
  let subChosen = urlNum(search.sub) != null;
  /** The current subtitle state came from the server's preferences (shows "Auto"). */
  const [subAuto, setSubAuto] = createSignal(false);
  const [selection, setSelection] = createSignal<Selected>();
  const [quick, setQuick] = createSignal<(QuickSave & { saved?: boolean; busy?: boolean })>();
  void loadPrefs();

  let video!: HTMLVideoElement;
  let root!: HTMLDivElement;
  let playBtn!: HTMLButtonElement;
  let scrubApi: ScrubberApi | undefined;
  const [osd, flash] = useOsd();
  const [speed, setSpeed] = createSignal(1);
  const [volume, setVolume] = createSignal(readVolume());
  const [muted, setMuted] = createSignal(false);
  const [pipOk, setPipOk] = createSignal(false);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let tickTimer: ReturnType<typeof setInterval> | undefined;
  let adapter: PlayerAdapter | undefined;
  let offs: (() => void)[] = [];
  let closed = false;
  let timing = props.startup;
  let initialTiming = true;
  let timingCleanup: (() => void) | undefined;

  const poke = () => {
    setVisible(true);
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (playing() && !picker() && scrub() == null && !info() && !quick() && !castSheet() && !speedSheet() && !qualitySheet()) setVisible(false);
    }, HIDE_AFTER);
  };

  // A touch/pen tap on the picture (and any click in TV mode) shows the controls, or hides them
  // if they're up; it never plays/pauses. A mouse click plays/pauses (see onPictureClick, #183).
  const tap = createTapToggle({
    shown: visible,
    show: poke,
    hide: () => {
      if (hideTimer) clearTimeout(hideTimer);
      if (scrub() == null) setVisible(false);
    },
  });

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
    api.progress(s.session_id, { position_s: pos(), paused }, keepalive).catch((e) => {
      // 404: the server ended the session (idle reap, restart). Open a new one at the same spot.
      if (e instanceof ApiError && e.status === 404 && session()?.session_id === s.session_id) sessionGone();
    });
  };
  /** The session is gone on the server (#95): reopen now if playing, or on the next play if paused. */
  const sessionGone = () => {
    if (gone || closed || localStopped || cast()) return;
    gone = true;
    if (held) return;
    setRecovering("Reconnecting to the server");
    void start(pos());
  };
  /** The viewer pressed play. A session that's gone is reopened at the playhead instead. */
  const userPlay = () => {
    held = false;
    if (gone) {
      void start(pos());
      return;
    }
    adapter?.play().catch(() => undefined);
  };
  const userPause = () => {
    held = true;
    adapter?.pause();
  };
  const endSession = (keepalive = false) => {
    const s = session();
    if (!s) return;
    report(true, keepalive);
    api.stopPlayback(s.session_id, keepalive).catch(() => undefined);
    setSession(undefined);
  };

  /** Open a session (library decide or the page's source). */
  const openSession = (o: CastOpen, forCast?: "chromecast" | "airplay"): Promise<DecideResponse> => {
    // Cast receivers get the server's Auto for their own profile.
    const q = forCast ? undefined : quality();
    // Multi-variant only where hls.js plays it (not TVs, Safari or casts: Q6) and for library
    // files (IPTV VOD stays single-variant: Q5).
    const adaptive = !forCast && !props.source && playsHlsWithHlsJs(video) && singleFor !== quality();
    return props.source
      ? props.source.decide({ audio: o.audio, subtitle: o.subtitle ?? -1, subtitleIndex: o.subtitle, start: o.start, forCast, quality: q })
      : decide(file!.id, { itemId: creditItem(file!, props.item.id), audio: o.audio, subtitle: o.subtitle, start: o.start, forCast, quality: q, adaptive });
  };

  /** (Re)start playback: decide → load. Used on open and when audio/burn-in subs change. */
  const start = async (at: number, forCast?: "airplay") => {
    if (!file) return;
    timingCleanup?.();
    timingCleanup = undefined;
    if (!initialTiming) {
      timing?.finish("superseded");
      timing = startupTiming(props.source ? "vod" : "library");
    }
    initialTiming = false;
    const trace = timing;
    trace?.mark("decide");
    setError(undefined);
    // The clock shows the start point until the first timeupdate (slow starts, remote seek).
    setPos(at);
    setBuffering(true);
    gone = false;
    probed = false;
    stalls.settle();
    endSession();
    let d: DecideResponse;
    try {
      d = await openSession({ audio: audio(), subtitle: subChosen ? sub() : null, start: at }, forCast);
      if (forCast && !/^https?:/i.test(d.url)) {
        // Source didn't honour for_cast: convert the session for the receiver.
        const c = await casting.castUrl(d.session_id);
        d = { ...d, url: c.url };
      }
    } catch (e) {
      if (trace === timing) trace?.finish("failed");
      setBuffering(false);
      setRecovering(undefined);
      setError(props.source?.error?.(e) ?? decideError(e));
      return;
    }
    if (closed) {
      api.stopPlayback(d.session_id).catch(() => undefined);
      return;
    }
    if (trace === timing) trace?.decided(d.decision.mode || d.mode);
    // "Original" means no video re-encode (#87). When the server can't avoid one (codec, HDR,
    // picture subtitles, the user's bitrate limit), say why and offer the transcode instead.
    if (!forCast && quality() === "original" && videoTranscoded(d)) {
      api.stopPlayback(d.session_id).catch(() => undefined);
      trace?.finish("canceled");
      adapter?.pause();
      setBuffering(false);
      setRecovering(undefined);
      setError({
        title: "Can't play the original here",
        body: `${whyTranscode(d)} Reel can convert it for you instead.`,
        retry: false,
        action: { label: "Play transcoded", icon: "play", busy: "Starting…", run: async () => void setQuality("auto") },
      });
      return;
    }
    // A rung the master doesn't list (left out for room, or too close to the original): play that
    // rung on its own, as #87 did.
    const select = isAdaptive(d) ? variantSelect(quality(), d.variants) : undefined;
    if (!forCast && select === null) {
      singleFor = quality();
      api.stopPlayback(d.session_id).catch(() => undefined);
      return start(at, forCast);
    }
    setSession(d);
    setVariantId(undefined);
    if (qualityOptions(d).length) setQualityOk(true);
    const sel = selectedOf(d);
    if (sel) {
      setSelection(sel);
      if (audio() == null && sel.audio_index != null) setAudio(sel.audio_index);
      if (!subChosen) {
        setSub(sel.subtitle_index ?? -1);
        setSubAuto(true);
      }
    }
    subChosen = true;
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
      .map((s) => ({ id: String(s.index), url: mediaUrl(s.url)!, label: subLabel(s).short, language: s.language ?? undefined }));
    try {
      // Paused by the viewer (a reopened session, a track change): load it, but stay paused (#100).
      if (trace && trace === timing) {
        trace.mark("load");
        let frame: number | undefined;
        const usable = () => {
          if (trace !== timing) return;
          trace.mark("usable");
          if (typeof video.requestVideoFrameCallback === "function") {
            frame = video.requestVideoFrameCallback(() => {
              if (trace === timing) {
                trace.mark("frame");
                trace.finish("rendered");
              }
            });
          }
        };
        video.addEventListener("loadeddata", usable, { once: true });
        timingCleanup = () => {
          video.removeEventListener("loadeddata", usable);
          if (frame !== undefined) video.cancelVideoFrameCallback(frame);
        };
      }
      const est = bandwidthEstimate();
      const variants = isAdaptive(d)
        ? { start: d.start_variant, estimateBps: est ? est * 1000 : undefined, select: select ?? undefined }
        : undefined;
      await a.load(mediaUrl(d.url)!, { startPositionSec: at || undefined, autoplay: !held, textTracks, variants });
      if (held) {
        setPlaying(false);
        setBuffering(false);
        setRecovering(undefined);
      }
    } catch {
      // Autoplay blocked: show the play button instead.
      setPlaying(false);
      setBuffering(false);
    }
    attachTracks(a, d.subtitles);
    if (a.getTracks) syncNativeAudio(a, a.getTracks());
    applySubtitle(d);
  };

  let applyingNativeAudio = false;
  function syncNativeAudio(a: PlayerAdapter, tracks: PlayerTracks): void {
    if (a !== adapter) return;
    setNativeTracks(tracks);
    const d = session();
    if (!d || !a.selectAudioTrack || applyingNativeAudio) return;
    const id = nativeAudioChoice(d.mode, d.decision.audio.type, d.audio, audio(), tracks.audio);
    if (id) {
      applyingNativeAudio = true;
      try { a.selectAudioTrack(id); } finally { applyingNativeAudio = false; }
    }
  }

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
        // An automatic restart is over once the picture moves.
        if (recovering() && t.positionSec > startAt + 0.2 && !video.paused) setRecovering(undefined);
        // Direct play: hls.js isn't there to measure, so refresh a stale estimate with a short probe.
        const s = session();
        if (!probed && s?.mode === "direct_play" && t.positionSec > startAt + 8 && playsHlsWithHlsJs(video) && probeDue()) {
          probed = true;
          void probeThroughput(mediaUrl(s.url)!);
        }
        setPos(t.positionSec);
        if (t.durationSec && isFinite(t.durationSec)) setDur(t.durationSec);
      }),
      a.on("ready", (r) => r.durationSec && isFinite(r.durationSec) && setDur(r.durationSec)),
      a.on("buffered", (b) => setBuffered(b.endSec)),
      a.on("play", () => {
        setPlaying(true);
        setBuffering(false);
        held = false;
        // Back from a long pause: make sure the server still has the session (a 404 reopens it).
        if (pausedAt && Date.now() - pausedAt > RESUME_CHECK_MS) report(false);
        pausedAt = 0;
        poke();
      }),
      a.on("pause", () => {
        setPlaying(false);
        setVisible(true);
        // A pause the player causes while it recovers isn't the viewer's.
        if (!recovering()) held = true;
        if (!pausedAt) pausedAt = Date.now();
        report(true);
      }),
      a.on("buffering", (b) => {
        setBuffering(b);
        if (b) directStall();
      }),
      a.on("variant", (v) => setVariantId(v.id)),
      a.on("throughput", (t) => recordThroughput(t.kbps)),
      a.on("ended", () => {
        setPlaying(false);
        setVisible(true);
        report(true);
      }),
      // Tizen AVPlay can't show <track>s: the adapter emits cues and we draw them.
      a.on("cue", (c) => setCueHtml(c.text ? cueToHtml(c.text) : "")),
      a.on("tracks", (t) => syncNativeAudio(a, t)),
      a.on("recovering", (r) => setRecovering(r ? r.reason : undefined)),
      a.on("error", (e) => {
        if (e.fatal) {
          if (a === adapter) timing?.finish("failed");
          setRecovering(undefined);
          // The player's own retries are spent. The server session may be gone too (reaped while
          // the stream was stuck): open a fresh one at the same spot, once a minute at most.
          if (Date.now() - autoRestartAt > 60_000 && !closed) {
            autoRestartAt = Date.now();
            setRecovering("Reconnecting to the server");
            void start(pos() || startAt);
            return;
          }
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

  /**
   * Direct play under Auto stalled (#1): two waits in 30 s mean the link can't carry the file.
   * Hold the estimate below the file's bitrate and re-decide from the playhead: the server then
   * remuxes it with rungs (Q1) and hls.js adapts. Once per play.
   */
  function directStall(): void {
    const s = session();
    if (!s || s.mode !== "direct_play" || directFallback || quality() !== "auto" || cast() || props.source) return;
    if (held || video.paused || !playing() || !playsHlsWithHlsJs(video)) return;
    if (!stalls.stall()) return;
    directFallback = true;
    capEstimate(s.bitrate_kbps ?? (file?.size && file.duration_s ? (file.size * 8) / file.duration_s / 1000 : 0));
    flash({ icon: "quality", text: "Switching to adaptive quality" });
    void start(pos());
  }

  // ---- subtitles -----------------------------------------------------------
  // One renderer for every platform: text tracks run in `hidden` mode (cues
  // still fire, nothing is drawn natively) and the active cues are drawn in our
  // own overlay — same size/position everywhere, clear of the controls, and
  // unified with the Tizen AVPlay `cue` event path.

  /** Web (hls.js / direct): sidecar VTT as <track> children. Native adapters take `textTracks`. */
  function attachTracks(a: PlayerAdapter, subs: SubtitleOption[]): void {
    if (a.selectTextTrack) return;
    for (const s of subs) {
      if (!s.url) continue;
      const t = document.createElement("track");
      t.kind = "subtitles";
      t.label = subLabel(s).short;
      t.srclang = s.language ?? "";
      t.src = mediaUrl(s.url)!;
      t.setAttribute("data-index", String(s.index));
      t.addEventListener("load", () => syncTracks());
      video.appendChild(t);
    }
  }
  function clearTracks(): void {
    Array.prototype.slice.call(video.querySelectorAll("track")).forEach((t: Element) => t.remove());
    bindCueTrack(undefined);
    setCueHtml("");
  }

  let cueTrack: TextTrack | undefined;
  let offCue: (() => void) | undefined;
  function bindCueTrack(t: TextTrack | undefined): void {
    if (t === cueTrack) return;
    offCue?.();
    offCue = undefined;
    cueTrack = t;
    setCueHtml("");
    if (!t) return;
    const h = () => renderCues(t);
    t.addEventListener("cuechange", h);
    offCue = () => t.removeEventListener("cuechange", h);
    renderCues(t);
  }
  function renderCues(t: TextTrack): void {
    const active = t.activeCues;
    const parts: string[] = [];
    if (active) for (let i = 0; i < active.length; i++) parts.push(cueToHtml((active[i] as VTTCue).text ?? ""));
    setCueHtml(parts.filter(Boolean).join("<br>"));
  }

  /**
   * Exactly the chosen track is active, in `hidden` mode (we draw it). Re-runs on
   * `change`/`addtrack`/`load`: Chrome auto-enables same-language tracks, and
   * native adapters may switch a track to `showing` themselves.
   */
  function syncTracks(): void {
    if (!video) return;
    const want = session()?.decision.subtitle.type === "burn_in" ? -1 : sub();
    const tracks = video.textTracks;
    let chosen: TextTrack | undefined;
    if (!adapter?.selectTextTrack) {
      const els = video.querySelectorAll("track");
      for (let i = 0; i < els.length; i++) {
        const el = els[i] as HTMLTrackElement;
        const on = Number(el.getAttribute("data-index")) === want;
        // AirPlay: let Safari send the track to the TV; otherwise we draw it.
        const mode: TextTrackMode = on ? (airplaying() ? "showing" : "hidden") : "disabled";
        if (el.track.mode !== mode) el.track.mode = mode;
        if (on) chosen = el.track;
      }
    } else {
      // webOS <video>: whatever the adapter enabled, draw it ourselves instead.
      for (let i = 0; i < tracks.length; i++) {
        const t = tracks[i]!;
        if (t.mode === "showing") t.mode = "hidden";
        if (t.mode === "hidden" && want >= 0) chosen = t;
      }
    }
    bindCueTrack(chosen);
  }

  function applySubtitle(d: DecideResponse | undefined = session()): void {
    const want = sub();
    // Burned-in subs are part of the picture; nothing to toggle client-side.
    if (d?.decision.subtitle.type === "burn_in") {
      bindCueTrack(undefined);
      return;
    }
    if (adapter?.selectTextTrack) adapter.selectTextTrack(want >= 0 ? String(want) : null);
    if (want < 0) setCueHtml("");
    syncTracks();
  }

  // A failed start puts its card on screen: move the remote onto its first action.
  // Once it recovers, hand focus back to play (the card's buttons are gone).
  let hadError = false;
  createEffect(() => {
    const e = error();
    if (!e && !hadError) return;
    hadError = !!e;
    requestAnimationFrame(() => {
      if (e) {
        const el = root?.querySelector<HTMLElement>(".player__error [data-autofocus]");
        if (el) focusElement(el, { smooth: false });
      } else if (playBtn && (!document.activeElement || document.activeElement === document.body || !document.activeElement.isConnected)) {
        focusElement(playBtn, { smooth: false });
      }
    });
  });

  /** Keep subtitles above the control bar while it's up (measured, not guessed). */
  const measureLift = () => {
    const bar = scrubApi?.el();
    if (!root || !bar) return;
    const top = bar.getBoundingClientRect().top - root.getBoundingClientRect().top;
    setSubLift(Math.max(0, root.clientHeight - top + root.clientHeight * 0.045));
  };
  createEffect(() => {
    visible();
    playing();
    requestAnimationFrame(measureLift);
  });

  /** After a manual change: offer to make it the default ("Always use English subtitles?"). */
  let quickTimer: ReturnType<typeof setTimeout> | undefined;
  const offer = (q: QuickSave | undefined) => {
    if (quickTimer) clearTimeout(quickTimer);
    if (!q || prefsSupported() === false) return setQuick(undefined);
    setQuick(q);
    poke();
    quickTimer = setTimeout(() => setQuick(undefined), 14_000);
  };
  const saveQuick = async () => {
    const q = quick();
    if (!q || q.busy) return;
    setQuick({ ...q, busy: true });
    try {
      await updatePrefs(q.patch);
      setQuick({ ...q, saved: true });
      if (quickTimer) clearTimeout(quickTimer);
      quickTimer = setTimeout(() => setQuick(undefined), 2600);
    } catch (e) {
      setQuick(undefined);
      toast(e instanceof Error ? e.message : "Couldn't save your preference", "error");
    }
  };
  const audioLang = () => iso1(audioList().find((a) => a.index === audio())?.language);

  const chooseSub = (v: number, quiet = false) => {
    if (v === sub()) return;
    setSub(v);
    setSubAuto(false);
    subChosen = true;
    const p = playbackPrefs();
    if (p && !quiet) {
      const s = v >= 0 ? subList().find((x) => x.index === v) : null;
      const cur = audioList().find((a) => a.index === audio());
      const isDefault = !!cur && (cur.default || audioList()[0] === cur) && !audioList().some((a) => a.default && a !== cur);
      offer(subtitleQuickSave(s ? { ...s, codec: file?.subtitles.find((x) => x.index === v)?.codec } : null, p, audioLang(), isDefault));
    }
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
    const p = playbackPrefs();
    if (p) offer(audioQuickSave(audioList().find((a) => a.index === v), p, audioList()));
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

  // ---- quality (#87) -----------------------------------------------------------
  /**
   * Per-play pick. A multi-variant session switches locally (#111): Auto = ABR, Original = pin,
   * a rung = ABR capped at that size. Otherwise re-decide from the playhead (same device id → the
   * server replaces the session).
   */
  const chooseQuality = (q: Quality) => {
    if (q === quality() && !error()) return;
    setQuality(q);
    if (cast()) return;
    const d = session();
    if (!error() && isAdaptive(d)) {
      const sel = variantSelect(q, d!.variants);
      if (sel && adapter?.selectVariant?.(sel)) return;
      if (!sel) singleFor = q;
    }
    void start(pos() || resumeAt());
  };
  /** What is playing now: the variant hls.js plays, or the session's single rendition. */
  const playingQuality = (): Quality | undefined => {
    const s = session();
    if (!isAdaptive(s)) return s?.quality;
    return variantOf(s, variantId())?.id;
  };
  /** Compact value next to the icon: what is playing now. */
  const qualityValue = () => {
    const q = playingQuality();
    return q ? (q === "original" ? "Orig" : QUALITY_NAME[q]) : QUALITY_NAME[quality()];
  };
  const qualityDesc = () => {
    const limit = userLimitKbps();
    const dflt = `Default on this device: ${QUALITY_NAME[deviceQuality()]}.`;
    return limit ? `${dflt} Your account is limited to ${fmtMbps(limit)}.` : dflt;
  };

  // ---- casting ---------------------------------------------------------------
  // Chromecast / DLNA: the TV plays its own session and this page becomes the remote (local
  // playback stops). AirPlay: Safari routes this very <video> to the TV, so it keeps playing
  // here — re-opened with a receiver-reachable URL — under the same remote view.
  const castKey = props.source ? `vod:${props.item.id}` : `item:${props.item.id}`;
  const cast = () => castFor(castKey);
  const airplaying = () => cast()?.remote.kind === "airplay";
  const [castSheet, setCastSheet] = createSignal(false);
  const [castBusy, setCastBusy] = createSignal("");
  const [airAvail, setAirAvail] = createSignal<boolean>();
  const eyebrow = () =>
    props.source?.eyebrow ?? (props.item.kind === "episode" ? `${series() ?? ""}${series() ? " · " : ""}${episodeCode(props.item)}` : undefined);

  const audioTracks = (list: { index: number; codec: string; channels: number; language?: string | null; title?: string | null; default?: boolean }[]): CastTrack[] =>
    list.map((a) => {
      const l = audioLabel(a);
      return { index: a.index, title: l.short, detail: l.detail || undefined, language: a.language };
    });
  const subTracks = (list: ReturnType<typeof subList>): CastTrack[] =>
    list.map((x) => {
      const l = subLabel(x);
      return { index: x.index, title: l.short, detail: l.detail, language: x.language, forced: x.forced, url: x.url && /^https?:/i.test(x.url) ? x.url : null };
    });

  /** A session's URLs as the receiver needs them (absolute; converted if the source ignored for_cast). */
  const toStream = async (d: DecideResponse): Promise<CastStream> => {
    let url = d.url;
    let contentType: string | undefined;
    const subUrls = new Map<number, string>();
    if (!/^https?:/i.test(url)) {
      const c = await casting.castUrl(d.session_id);
      url = c.url;
      contentType = c.content_type;
      c.subtitles.forEach((x) => x.url && subUrls.set(x.index, x.url));
    }
    const hls = /\.m3u8(\?|$)/i.test(url) || /mpegurl/i.test(contentType ?? "");
    return {
      sessionId: d.session_id,
      url,
      contentType: hls ? "application/x-mpegURL" : contentType || "video/mp4",
      fmp4: hls, // our transcoder writes fMP4 segments
      live: false,
      audio: audioTracks(d.audio),
      subtitles: subTracks(d.subtitles).map((t) => ({ ...t, url: subUrls.get(t.index) ?? t.url })),
      selected: selectedOf(d),
    };
  };

  let localStopped = false;
  /** Hand playback to the TV: stop the local stream (frees the server's stream slot first). */
  const stopLocal = () => {
    if (localStopped) return;
    localStopped = true;
    offs.forEach((f) => f());
    offs = [];
    endSession();
    adapter?.destroy();
    adapter = undefined;
    clearTracks();
    setPlaying(false);
    setBuffering(false);
    setError(undefined);
    setRecovering(undefined);
    setVisible(true);
  };
  const resumeLocal = (at: number) => {
    localStopped = false;
    held = false;
    setPos(at);
    void start(at);
  };

  const castSource = (): CastSource => ({
    meta: {
      key: castKey,
      title: props.item.title,
      eyebrow: eyebrow(),
      poster: props.item.poster_url,
      backdrop: props.item.backdrop_url,
      duration: dur() || file?.duration_s,
    },
    open: async (target, o) => {
      if (target === "chromecast") stopLocal();
      return toStream(await openSession(o, target));
    },
    dlna: props.source
      ? props.source.dlna
      : (o) => ({
          kind: "item",
          id: file ? Number(file.id) : null,
          file_id: file ? Number(file.id) : null,
          start_s: o.start > 0 ? Math.floor(o.start) : null,
          audio_index: o.audio,
          subtitle_index: o.subtitle,
        }),
    audio: audioTracks(audioList()),
    subtitles: subTracks(subList()),
  });
  const castOpen = (): CastOpen => ({
    audio: audio(),
    subtitle: subChosen ? sub() : null,
    start: Math.max(0, scrub() ?? pos()) || resumeAt(),
  });

  const pickTarget = async (t: CastTarget) => {
    if (t.kind === "airplay") {
      // Safari's own picker; the switch is picked up by `webkitcurrentplaybacktargetiswirelesschanged`.
      (video as HTMLVideoElement & { webkitShowPlaybackTargetPicker?: () => void }).webkitShowPlaybackTargetPicker?.();
      return;
    }
    const o = castOpen();
    if (t.kind === "dlna" && file) {
      // The TV's own session can't report back the server's automatic pick: resolve it here
      // (same rules) so the remote's Audio / Subtitles buttons show the truth.
      const pv = playbackPrefs() && previewSelection(file.audio, file.subtitles, playbackPrefs()!);
      if (o.audio == null) o.audio = pv ? pv.audio : null;
      if (o.subtitle == null) o.subtitle = pv ? pv.sub ?? -1 : null;
    }
    setCastBusy(t.kind === "dlna" ? `Sending to ${t.name}…` : "Connecting to your Chromecast…");
    try {
      if (t.kind === "dlna") stopLocal();
      await startCast(t, castSource(), o);
      stopLocal();
    } catch (e) {
      if (!(e instanceof CastCancelled)) toast(e instanceof Error ? e.message : "Couldn't start casting", "error");
      if (localStopped && !cast()) resumeLocal(o.start);
    } finally {
      setCastBusy("");
    }
  };

  const onAirplayTarget = () => {
    const v = video as HTMLVideoElement & { webkitCurrentPlaybackTargetIsWireless?: boolean; disableRemotePlayback?: boolean };
    if (!v.webkitCurrentPlaybackTargetIsWireless || cast()) return;
    void startCast({ kind: "airplay", id: "airplay", name: "AirPlay" }, castSource(), castOpen(), {
      video,
      load: async (o) => {
        if (o.audio != null) setAudio(o.audio);
        if (o.subtitle != null) {
          setSub(o.subtitle);
          subChosen = true;
        }
        await start(o.start, "airplay");
      },
      stop: () => {
        // No "disconnect" API: disabling remote playback hands the route back to this device.
        v.disableRemotePlayback = true;
        setTimeout(() => (v.disableRemotePlayback = false), 1500);
      },
    }).then(() => start(pos(), "airplay"));
  };

  // ---- transport -------------------------------------------------------------
  const toggle = () => {
    if (!adapter) return;
    // Use our state, not video.paused: AVPlay doesn't drive the <video> element.
    if (!playing()) userPlay();
    else userPause();
    poke();
  };
  /** ±seconds from buttons / remote keys: previewed on the scrubber, committed shortly after. */
  const seekBy = (delta: number) => {
    scrubApi?.nudge(delta, true);
    poke();
  };
  const seekTo = (t: number) => {
    if (adapter) {
      stalls.settle();
      adapter.seek(t);
      setPos(t);
    }
    poke();
  };

  // ---- volume / speed / picture-in-picture (HTML <video> players; AVPlay has its own) ----
  const htmlVideo = () => !nativePlane();
  const applyVolume = (v: number, m = muted()) => {
    if (!video) return;
    video.volume = Math.max(0, Math.min(1, v));
    video.muted = m;
  };
  const changeVolume = (v: number) => {
    const n = Math.max(0, Math.min(1, v));
    setVolume(n);
    setMuted(n === 0);
    applyVolume(n, n === 0);
    try {
      localStorage.setItem(VOL_KEY, String(n));
    } catch {
      /* ignore */
    }
  };
  const toggleMute = () => {
    const m = !muted() && volume() > 0;
    setMuted(m);
    if (!m && volume() === 0) changeVolume(0.5);
    else applyVolume(volume(), m);
    flash({ icon: m ? "mute" : "volume", text: m ? "Muted" : `Volume ${Math.round(volume() * 100)}%` });
  };
  const changeSpeed = (v: number) => {
    setSpeed(v);
    if (video) {
      video.defaultPlaybackRate = v;
      video.playbackRate = v;
    }
    flash({ icon: "speed", text: v === 1 ? "Normal speed" : `${v}×` });
  };
  const [speedSheet, setSpeedSheet] = createSignal(false);
  const togglePip = () => {
    const v = video as HTMLVideoElement & { webkitSetPresentationMode?: (m: string) => void; webkitPresentationMode?: string };
    if (document.pictureInPictureElement) void document.exitPictureInPicture().catch(() => undefined);
    else if (document.pictureInPictureEnabled && v.requestPictureInPicture) void v.requestPictureInPicture().catch(() => undefined);
    else if (v.webkitSetPresentationMode) v.webkitSetPresentationMode(v.webkitPresentationMode === "picture-in-picture" ? "inline" : "picture-in-picture");
  };

  // ---- next episode (same season) ----
  const [nextEp] = createResource(
    () => (props.item.kind === "episode" && props.item.parent_id != null && !props.source ? props.item.parent_id : undefined),
    async (season) => {
      const eps = await api.children(season);
      const cur = props.item.episode_no ?? -1;
      return eps
        .filter((e) => e.kind === "episode" && (e.episode_no ?? -1) > cur)
        .sort((a, b) => (a.episode_no ?? 0) - (b.episode_no ?? 0))[0];
    },
  );
  const playNext = () => {
    const n = nextEp();
    if (n) navigate(toPath(playHref(n, { fromStart: !n.progress })), { replace: true });
  };

  /** "C": subtitles off ↔ back on (last choice, else the first text track). */
  let lastSub = -1;
  const toggleCaptions = () => {
    const list = subList();
    if (sub() >= 0) {
      lastSub = sub();
      chooseSub(-1, true);
      flash({ icon: "cc", text: "Subtitles off" });
      return;
    }
    const pick = list.find((x) => x.index === lastSub) ?? list.find((x) => x.url && !x.forced) ?? list[0];
    if (!pick) return flash({ icon: "cc", text: "No subtitles" });
    chooseSub(pick.index, true);
    flash({ icon: "cc", text: subLabel(pick).short });
  };

  const exit = () => {
    if (window.history.length > 1) window.history.back();
    else navigate(props.source ? props.source.back : `/item/${props.item.id}`);
  };
  // Full screen goes on the route's wrapper when there is one, so "Next" keeps it (#247).
  const fs = playerFullscreen({ doc: document, host: useContext(FullscreenHost), own: () => root });
  const fullscreen = fs.toggle;

  // ---- clicks and taps on the picture (#183, YouTube's split) ---------------------------
  // Mouse: a click plays/pauses at once with the centre flash; a double-click toggles full
  // screen and undoes that click, so playback ends where it started. Touch/pen: a tap
  // shows/hides the controls (createTapToggle); a double-tap on the left/right third seeks
  // ∓10 s and further taps add up. TV mode keeps every click on the controls path: the magic
  // remote's pointer reports "mouse", and OK/Space already pause there.
  let clickKind: "mouse" | "touch" | undefined;
  /** The play state a mouse click changed, and when: a double-click right after restores it. */
  let mouseUndo: { was: boolean; at: number } | undefined;
  const taps = createDoubleTap();
  const [ripple, setRipple] = createSignal<Extract<TapResult, { kind: "seek" }> & { key: number }>();
  let rippleTimer: ReturnType<typeof setTimeout> | undefined;
  let rippleKey = 0;
  onCleanup(() => rippleTimer && clearTimeout(rippleTimer));
  const onPictureClick = (e: MouseEvent) => {
    const kind = clickKind ?? pointerKind((e as PointerEvent).pointerType);
    clickKind = undefined;
    if (TV || e.detail === 0) return tap.onClick(e);
    if (kind === "mouse") {
      tap.cancel();
      if (e.detail > 1) return; // second click of a double-click: onDblClick owns it
      const was = playing();
      toggle();
      flash({ icon: was ? "pause" : "play" });
      mouseUndo = { was, at: Date.now() };
      return;
    }
    const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const r = taps.tap(Date.now(), zoneOf(e.clientX - box.left, box.width));
    if (r.kind === "single") return tap.onClick(e);
    tap.cancel();
    if (r.kind === "double") return fullscreen();
    seekBy(r.delta);
    setRipple({ ...r, key: ++rippleKey });
    if (rippleTimer) clearTimeout(rippleTimer);
    rippleTimer = setTimeout(() => setRipple(undefined), 750);
  };
  const onPictureDblClick = () => {
    if (TV) {
      tap.cancel();
      return fullscreen();
    }
    // Touch double-taps are handled in onPictureClick (browsers differ on firing dblclick for them).
    const undo = mouseUndo;
    mouseUndo = undefined;
    if (!undo || Date.now() - undo.at > 1000) return;
    fullscreen();
    if (undo.was) userPlay();
    else userPause();
  };

  onMount(() => {
    if (!file) {
      setError({ title: "Nothing to play", body: "This title has no playable file yet.", retry: false });
      setBuffering(false);
      return;
    }
    // "Play on…" from the detail page lands here with ?cast=…; a cast of this title that's
    // already running re-attaches the remote view instead of playing locally.
    const castParam = search.cast;
    if (castParam) setSearch({ cast: undefined }, { replace: true });
    if (cast()) {
      localStopped = true;
      setBuffering(false);
    } else if (castParam === "chromecast" || (castParam && castParam.indexOf("dlna:") === 0)) {
      localStopped = true;
      setBuffering(false);
      void (async () => {
        await loadPrefs();
        let t: CastTarget = { kind: "chromecast", id: "chromecast", name: "Chromecast" };
        if (castParam !== "chromecast") {
          const id = castParam.slice(5);
          const d = (await casting.devices().catch(() => [])).find((x) => x.id === id);
          t = { kind: "dlna", id, name: d?.name ?? "your TV", device: d };
        }
        await pickTarget(t);
        if (!cast() && !session()) resumeLocal(resumeAt());
      })();
    } else {
      void start(resumeAt());
      if (castParam === "airplay") setCastSheet(true);
    }
    const offCastEnd = onCastEnd((a, reason, at) => {
      if (a.source.meta.key !== castKey) return;
      if (reason === "ended" && a.remote.kind !== "airplay") return exit();
      if (a.remote.kind === "airplay") {
        // Back from the TV: reopen with this browser's URL and profile.
        void start(at || pos());
        return;
      }
      resumeLocal(at);
    });
    const onAirAvail = (e: Event) => setAirAvail((e as Event & { availability?: string }).availability === "available");
    video.addEventListener("webkitplaybacktargetavailabilitychanged", onAirAvail);
    video.addEventListener("webkitcurrentplaybacktargetiswirelesschanged", onAirplayTarget);
    onCleanup(() => {
      offCastEnd();
      video.removeEventListener("webkitplaybacktargetavailabilitychanged", onAirAvail);
      video.removeEventListener("webkitcurrentplaybacktargetiswirelesschanged", onAirplayTarget);
      // AirPlay rides on this page's <video>: it ends with the page.
      if (airplaying()) void stopCast();
      if (quickTimer) clearTimeout(quickTimer);
    });
    // Tracks added by native adapters (webOS) get the same placement once their cues load.
    const onAddTrack = () => syncTracks();
    video.textTracks.addEventListener("addtrack", onAddTrack as EventListener);
    video.textTracks.addEventListener("change", syncTracks);
    window.addEventListener("resize", measureLift);
    onCleanup(() => {
      video.textTracks.removeEventListener("addtrack", onAddTrack as EventListener);
      video.textTracks.removeEventListener("change", syncTracks);
      window.removeEventListener("resize", measureLift);
      offCue?.();
    });
    focusElement(playBtn, { smooth: false });
    poke();
    // Volume survives sessions on this device; the element reports changes from anywhere.
    applyVolume(volume(), false);
    const onVol = () => {
      setMuted(video.muted || video.volume === 0);
      if (video.volume > 0) setVolume(video.volume);
    };
    // A new source resets the rate: keep the chosen speed.
    const onRate = () => {
      if (htmlVideo() && video.playbackRate !== speed()) {
        video.defaultPlaybackRate = speed();
        video.playbackRate = speed();
      }
    };
    video.addEventListener("volumechange", onVol);
    video.addEventListener("loadedmetadata", onRate);
    const pv = video as HTMLVideoElement & { webkitSupportsPresentationMode?: (m: string) => boolean };
    setPipOk(!TV && (!!document.pictureInPictureEnabled || !!pv.webkitSupportsPresentationMode?.("picture-in-picture")));
    onCleanup(() => {
      video.removeEventListener("volumechange", onVol);
      video.removeEventListener("loadedmetadata", onRate);
    });

    tickTimer = setInterval(() => {
      if (playing()) report(false);
      // Paused: keep the session alive (a paused player with a full buffer requests nothing, #95).
      else if (session() && pausedAt && Date.now() - pausedAt < PAUSED_KEEPALIVE_MS) report(true);
    }, TICK_MS);

    const onKey = (e: KeyboardEvent) => {
      if (picker() || castSheet() || speedSheet() || qualitySheet()) return;
      const k = e.key;
      const c = cast();
      if (c) {
        // The remote view owns the screen: only Space/K reach the TV from anywhere.
        if (k === " " || k === "k") {
          e.preventDefault();
          if (c.remote.state() === "playing") c.remote.pause();
          else c.remote.play();
        }
        return;
      }
      // Back is handled by onBack below; don't let it re-show the controls first.
      if (k === "Escape" || k === "Backspace" || k === "GoBack" || e.keyCode === 461 || e.keyCode === 10009) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const hidden = !visible();
      const inSlider = !!(document.activeElement as HTMLElement | null)?.closest?.(".pscrub__bar, .pvol__bar");
      const take = () => {
        e.preventDefault();
        e.stopImmediatePropagation();
      };
      if (k === " " || k === "k") {
        // Space on a focused button presses it; anywhere else it's play/pause.
        if (k === " " && !hidden && (document.activeElement as HTMLElement | null)?.tagName === "BUTTON") return poke();
        take();
        const was = playing();
        toggle();
        flash({ icon: was ? "pause" : "play" });
        return;
      }
      if (k === "i") {
        setInfo((v) => !v);
        return poke();
      }
      if (k === "f" && !TV) return fullscreen();
      if (k === "m" && htmlVideo()) return toggleMute();
      if (k === "c") {
        toggleCaptions();
        return poke();
      }
      if ((k === "j" || k === "l") && !TV) {
        seekBy(k === "j" ? -10 : 10);
        return flash({ icon: k === "j" ? "replay" : "skip", text: k === "j" ? "−10s" : "+10s" });
      }
      if (k === "ArrowLeft" || k === "ArrowRight") {
        const dir = k === "ArrowLeft" ? -1 : 1;
        if (!TV && !inSlider) {
          // Desktop: arrows always seek (Shift = 30 s); Tab moves focus.
          take();
          const d = dir * (e.shiftKey ? 30 : 10);
          seekBy(d);
          flash({ icon: dir < 0 ? "replay" : "skip", text: `${d < 0 ? "−" : "+"}${Math.abs(d)}s` });
          return;
        }
        if (TV && hidden) {
          // Remote with the controls hidden: straight into seek mode on the timeline.
          take();
          scrubApi?.nudge(dir * 10);
          poke();
          const bar = scrubApi?.el();
          if (bar) focusElement(bar, { smooth: false });
          return;
        }
      }
      if ((k === "ArrowUp" || k === "ArrowDown") && !TV && htmlVideo()) {
        take();
        changeVolume(volume() * (muted() ? 0 : 1) + (k === "ArrowUp" ? 0.05 : -0.05));
        flash({ icon: volume() === 0 ? "mute" : "volume", text: `Volume ${Math.round(volume() * 100)}%` });
        return poke();
      }
      if (hidden && /^(ArrowUp|ArrowDown|ArrowLeft|ArrowRight|Enter)$/.test(k)) {
        // First press only wakes the controls (focus stays where it was).
        take();
        return poke();
      }
      poke();
    };
    window.addEventListener("keydown", onKey, true);
    // Remote transport keys (normalised by the platform layer). Digits and
    // channel up/down stay unhandled here: reserved for Live TV.
    const offMedia = onRemoteKey((e) => {
      const m = e.key;
      if (m !== "play" && m !== "pause" && m !== "playpause" && m !== "ff" && m !== "rw" && m !== "stop" && m !== "info") return false;
      const c = cast();
      if (c) {
        const r = c.remote;
        if (m === "playpause") (r.state() === "playing" ? r.pause() : r.play());
        else if (m === "play") r.play();
        else if (m === "pause") r.pause();
        else if (m === "ff" && !c.source.meta.live) r.seek(r.position() + 30);
        else if (m === "rw" && !c.source.meta.live) r.seek(Math.max(0, r.position() - 10));
        else if (m === "stop") void stopCast();
        return true;
      }
      poke();
      if (!adapter) return true;
      if (m === "playpause") toggle();
      else if (m === "play") userPlay();
      else if (m === "pause") userPause();
      else if (m === "ff") seekBy(30);
      else if (m === "rw") seekBy(-10);
      else if (m === "stop") exit();
      else if (m === "info") setInfo((v) => !v);
      return true;
    });
    const offBack = onBack(() => {
      if (picker() || castSheet() || speedSheet() || qualitySheet()) return false;
      // Back peels one layer at a time: seek preview → info → controls → leave.
      if (scrubApi?.cancel()) {
        poke();
        return true;
      }
      if (cast()) {
        // Leave the remote view; the TV keeps playing (come back to this title to control it).
        exit();
        return true;
      }
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
      timingCleanup?.();
      timing?.finish("canceled");
      closed = true;
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pagehide", onUnload);
      offMedia();
      offBack();
      if (hideTimer) clearTimeout(hideTimer);
      if (tickTimer) clearInterval(tickTimer);
      endSession(true);
      offs.forEach((f) => f());
      adapter?.destroy();
      invalidate("home");
      invalidate("item");
      props.source?.onClose?.();
      fs.release();
    });
  });

  const bufferedEnd = () => buffered();
  const audioList = () => session()?.audio ?? file?.audio ?? [];
  /** Session list (has VTT urls); falls back to the file's streams while (re)deciding. */
  const subList = (): { index: number; language?: string | null; title?: string | null; forced: boolean; external: boolean; url?: string | null }[] =>
    session()?.subtitles ?? file?.subtitles ?? [];
  /** Compact choices for the control bar: "EN · 5.1", "EN SDH", "Off". */
  const CH: Record<number, string> = { 1: "1.0", 2: "2.0", 6: "5.1", 8: "7.1" };
  const audioShort = () => {
    const a = audioList().find((x) => x.index === audio());
    if (!a) return "Default";
    const lang = iso1(a.language).toUpperCase() || codecShort(a.codec);
    return `${lang} · ${CH[a.channels] ?? `${a.channels}ch`}`;
  };
  const audioText = () => {
    const a = audioList().find((x) => x.index === audio());
    return a ? audioLabel(a).short : "Default";
  };
  const subShort = () => {
    const x = subList().find((y) => y.index === sub());
    if (!x) return "Off";
    const lang = iso1(x.language).toUpperCase() || subLabel(x).short.slice(0, 2).toUpperCase();
    return `${lang}${isSdh(x) ? " SDH" : ""}${x.forced ? " Forced" : ""}`;
  };
  const subText = () => {
    const x = subList().find((y) => y.index === sub());
    return x ? subLabel(x).short : "Off";
  };
  const mode = () => session()?.mode;
  const transcoding = () => mode() === "transcode" || mode() === "audio_transcode";
  const shownPos = () => scrub() ?? pos();
  /** "Ends at 22:41" — when this would finish at the current speed. */
  const endsAt = () => {
    const d = dur();
    if (!d || !session()) return "";
    const t = new Date(Date.now() + (Math.max(0, d - shownPos()) * 1000) / speed());
    return t.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  };
  const topEyebrow = () =>
    props.source?.eyebrow ??
    (props.item.kind === "episode"
      ? `${series() ?? ""}${series() ? " · " : ""}${episodeCode(props.item)}`
      : [props.item.kind === "movie" ? "Film" : "", props.item.year ? String(props.item.year) : ""].filter(Boolean).join(" · "));

  return (
    <div
      ref={root}
      class="player__stage"
      classList={{
        "is-chrome": visible() || !playing() || scrub() != null,
        "is-paused": !playing(),
        "is-native-plane": nativePlane(),
        "is-seekmode": scrub() != null,
      }}
      onMouseMove={poke}
      onMouseLeave={() => {
        if (!playing() || picker() || castSheet() || speedSheet() || qualitySheet()) return;
        if (hideTimer) clearTimeout(hideTimer);
        hideTimer = setTimeout(() => playing() && scrub() == null && setVisible(false), 700);
      }}
    >
      <video
        ref={video}
        class="player__video"
        playsinline
        preload="auto"
        crossorigin={getServerUrl() ? "anonymous" : undefined}
        onPointerDown={(e) => {
          clickKind = pointerKind(e.pointerType);
          tap.onPointerDown();
        }}
        onClick={onPictureClick}
        onDblClick={onPictureDblClick}
      />

      <Osd osd={osd()} />
      <Show when={ripple()} keyed>
        {(r) => (
          <div class="pripple" classList={{ "pripple--back": r.total < 0 }} aria-hidden="true">
            <Icon name={r.total < 0 ? "replay" : "skip"} />
            <span>{`${r.total < 0 ? "−" : "+"}${Math.abs(r.total)} s`}</span>
          </div>
        )}
      </Show>

      <Show when={cueHtml()}>
        <div
          class="subs"
          classList={{ "is-lifted": visible() || !playing() }}
          style={{ "--sub-lift": `${subLift()}px` }}
          aria-live="off"
        >
          <span class="subs__text" innerHTML={cueHtml()} />
        </div>
      </Show>

      <Show when={(buffering() || recovering()) && !error()}>
        <div class="player__buffer" aria-label={recovering() ?? "Buffering"}>
          <span class="spinner" />
          <Show when={recovering()}>{(r) => <span class="player__recovering" role="status">{r()}…</span>}</Show>
        </div>
      </Show>

      <Show when={error()}>
        {(e) => (
          <div class="player__error" role="alert">
            <Icon name="alert" />
            <h2>{e().title}</h2>
            <p>{e().body}</p>
            <div class="actions actions--center">
              <Show when={e().action}>
                {(a) => <ActionButton label={a().label} run={a().run} icon={a().icon} busy={a().busy} onDone={() => {
                  held = false;
                  void start(pos() || resumeAt());
                }} />}
              </Show>
              <Show when={e().retry}>
                <button type="button" class={e().action ? "btn" : "btn btn--primary"} data-autofocus={e().action ? undefined : ""} onClick={() => {
                  held = false;
                  void start(pos() || resumeAt());
                }}>
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
        {(s) => (
          <StreamInfo
            session={s()}
            file={file}
            selection={selection()}
            asked={quality()}
            playing={variantOf(s(), variantId())}
            estimate={bandwidthEstimate()}
            onClose={() => setInfo(false)}
          />
        )}
      </Show>

      <Show when={castBusy()}>
        <div class="player__buffer player__castbusy" role="status">
          <span class="spinner" />
          <p>{castBusy()}</p>
        </div>
      </Show>

      <Show when={!cast() && quick()}>
        {(q) => (
          <div class="quicksave" role="status" data-nav-group>
            <p class="quicksave__text">
              {q().saved ? "Saved. Change it any time in Settings → Playback." : q().text}
              <Show when={!q().saved}><small>Just for you, on every device.</small></Show>
            </p>
            <Show when={!q().saved}>
              <button type="button" class="btn btn--primary btn--sm" aria-busy={q().busy} onClick={() => void saveQuick()}>
                {q().busy ? "Saving…" : "Save"}
              </button>
              <button type="button" class="btn btn--quiet btn--sm" onClick={() => setQuick(undefined)}>
                Not now
              </button>
            </Show>
          </div>
        )}
      </Show>

      <div class="pc" data-nav-fixed aria-hidden={!visible() && playing() && scrub() == null ? "true" : undefined}>
        <div class="pc__scrim pc__scrim--top" />
        <div class="pc__scrim pc__scrim--bottom" />

        <header class="pc__top">
          <CtlButton icon="back" label="Back" onClick={exit} />
          <div class="pc__titles">
            <Show when={topEyebrow()}>
              <p class="pc__eyebrow">{topEyebrow()}</p>
            </Show>
            <h1 class="pc__title">{props.item.title}</h1>
          </div>
          <div class="pc__meta">
            <Show when={endsAt()}>
              <span class="pc__ends">Ends at {endsAt()}</span>
            </Show>
            <Show when={mode()}>
              <button
                type="button"
                class="pc__badge"
                classList={{ "is-transcode": transcoding() }}
                aria-pressed={info()}
                title="Stream info (I)"
                onClick={() => setInfo((v) => !v)}
              >
                <i aria-hidden="true" />
                {transcoding() ? (mode() === "audio_transcode" ? "Audio transcode" : "Transcoding") : MODE_LABEL[mode()!] ?? mode()}
              </button>
            </Show>
          </div>
        </header>

        <div class="pc__bottom">
          <Scrubber
            position={pos()}
            duration={dur()}
            buffered={bufferedEnd()}
            onSeek={seekTo}
            onPreview={(t) => {
              setScrub(t);
              poke();
            }}
            onActivity={poke}
            api={(a) => (scrubApi = a)}
          />

          {/* One group: coming down from the timeline lands on Play (or where you were). */}
          <div class="pc__row" data-nav-group>
            <div class="pc__side pc__side--start">
              <Show when={!TV && htmlVideo()}>
                <Volume value={volume()} muted={muted()} onChange={changeVolume} onMute={toggleMute} />
              </Show>
            </div>

            <Transport
              playing={playing()}
              onToggle={toggle}
              onBack={() => seekBy(-10)}
              onForward={() => seekBy(30)}
              next={nextEp() ? { label: `Next: ${episodeCode(nextEp()!)}`, onClick: playNext } : undefined}
              playRef={(el) => (playBtn = el)}
            />

            <div class="pc__side pc__side--end">
              <CtlButton class="pc__audio" icon="audio" label={`Audio · ${audioText()}`} value={audioShort()} disabled={audioList().length < 2} onClick={() => setPicker("audio")} />
              <CtlButton
                class="pc__subs"
                icon="cc"
                label={subAuto() ? `Subtitles · ${subText()} (automatic)` : `Subtitles · ${subText()}`}
                value={subShort()}
                tag={subAuto() && sub() >= 0 ? "Auto" : undefined}
                active={sub() >= 0}
                disabled={!subList().length}
                onClick={() => setPicker("sub")}
              />
              <Show when={qualityOk()}>
                <CtlButton
                  class="pc__quality"
                  icon="quality"
                  label={`Quality · ${QUALITY_NAME[quality()]}${playingQuality() ? `, playing ${QUALITY_NAME[playingQuality()!]}` : ""}`}
                  value={qualityValue()}
                  tag={quality() === "auto" ? "Auto" : undefined}
                  onClick={() => session() && setQualitySheet(true)}
                />
              </Show>
              <Show when={htmlVideo()}>
                <CtlButton class="pc__speed" icon="speed" label="Playback speed" value={speed() !== 1 ? `${speed()}×` : undefined} active={speed() !== 1} onClick={() => setSpeedSheet(true)} />
              </Show>
              <span class="pc__sep" aria-hidden="true" />
              <Show when={!TV}>
                <CtlButton
                  icon={airplaying() ? "airplay" : "cast"}
                  label={cast() ? `Playing on ${cast()!.remote.device()}` : "Play on TV"}
                  active={!!cast()}
                  onClick={() => setCastSheet(true)}
                />
              </Show>
              <CtlButton class="pc__info" icon="info" label="Stream info" pressed={info()} onClick={() => setInfo((v) => !v)} />
              <Show when={pipOk() && htmlVideo()}>
                <CtlButton class="pc__pip" icon="pip" label="Picture in picture" onClick={togglePip} />
              </Show>
              <Show when={!TV}>
                <CtlButton icon="fullscreen" label="Full screen (F)" onClick={fullscreen} />
              </Show>
            </div>
          </div>
        </div>
      </div>

      <Show when={qualitySheet()}>
        <Sheet
          title="Quality"
          selected={quality()}
          description={qualityDesc()}
          options={QUALITIES.map((q) => {
            const o = qualityOptions(session()).find((x) => x.id === q);
            return {
              value: q,
              title: q === "original" ? "Original — no re-encoding" : QUALITY_NAME[q],
              detail: optionDetail(o, q, session(), playingQuality()),
              disabled: q !== "auto" && (!o || !o.available) && q !== quality(),
              tag: q !== "auto" && playingQuality() === q ? "Now" : undefined,
            };
          })}
          onSelect={chooseQuality}
          onClose={() => {
            setQualitySheet(false);
            poke();
          }}
          footer={
            <a class="sheet__link" href={href("/settings/playback")}>
              <Icon name="gear" /> Default quality for this device
            </a>
          }
        />
      </Show>

      <Show when={speedSheet()}>
        <Sheet
          title="Playback speed"
          selected={speed()}
          options={SPEEDS.map((v) => ({ value: v, title: v === 1 ? "Normal" : `${v}×`, detail: v === 1 ? "1×" : undefined }))}
          onSelect={changeSpeed}
          onClose={() => {
            setSpeedSheet(false);
            poke();
          }}
        />
      </Show>

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
            subList().map((s) => {
              const l = subLabel(s);
              // No VTT url in the session = image subs: they're burned in (forces a transcode).
              const burn = session() && !s.url ? " · burned in (transcodes)" : "";
              return { value: s.index, title: l.title, detail: l.detail + burn };
            }),
          )}
          onSelect={chooseSub}
          onClose={() => {
            setPicker(null);
            poke();
          }}
          description={subAuto() && sub() >= 0 ? "Turned on automatically from your playback settings." : undefined}
          footer={
            <a class="sheet__link" href={href("/settings/playback")}>
              <Icon name="gear" /> Subtitle & audio preferences
            </a>
          }
        />
      </Show>

      <Show when={cast()}>
        {(c) => (
          <CastRemote
            cast={c()}
            eyebrow={eyebrow()}
            onBack={exit}
            onDevices={() => setCastSheet(true)}
            onStop={() => void stopCast()}
          />
        )}
      </Show>

      <Show when={castSheet()}>
        <CastSheet
          current={cast()?.target.id}
          airplayAvailable={airAvail()}
          dlna={!!castSource().dlna}
          onLocal={cast() ? () => void stopCast() : undefined}
          onPick={(t) => void pickTarget(t)}
          onClose={() => {
            setCastSheet(false);
            poke();
          }}
        />
      </Show>
    </div>
  );
}

function readVolume(): number {
  try {
    const v = Number(localStorage.getItem(VOL_KEY));
    return localStorage.getItem(VOL_KEY) != null && isFinite(v) ? Math.max(0, Math.min(1, v)) : 1;
  } catch {
    return 1;
  }
}

/** Fix-it button on the error card: busy while it runs, then playback restarts. */
function ActionButton(props: { label: string; run: () => Promise<void>; icon?: IconName; busy?: string; onDone: () => void }): JSX.Element {
  const [busy, setBusy] = createSignal(false);
  const [failed, setFailed] = createSignal("");
  return (
    <>
      <button
        type="button"
        class="btn btn--primary"
        data-autofocus
        aria-busy={busy()}
        onClick={async () => {
          if (busy()) return;
          setBusy(true);
          setFailed("");
          try {
            await props.run();
            props.onDone();
          } catch (e) {
            setFailed(e instanceof Error ? e.message : "That didn't work");
          } finally {
            setBusy(false);
          }
        }}
      >
        <Icon name={busy() ? "clock" : props.icon ?? "x"} /> <span>{busy() ? props.busy ?? "Stopping…" : props.label}</span>
      </button>
      <Show when={failed()}>
        <p class="player__error-note" role="status">{failed()}</p>
      </Show>
    </>
  );
}

/** Sanitise a cue for innerHTML: escape everything, then re-allow <i>/<b>/<u>; drop VTT/ASS markup. */
function cueToHtml(text: string): string {
  const esc = text
    .replace(/\{\\[^}]*\}/g, "") // ASS override tags left in SRT ({\an8})
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  return esc
    .replace(/&lt;(\/?)(i|b|u)&gt;/gi, "<$1$2>")
    .replace(/&lt;\/?[^&]{0,40}?&gt;/g, "") // <c.yellow>, <v Speaker>, <00:01.000>, <font …>
    .replace(/\r?\n/g, "<br>")
    .trim();
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
function StreamInfo(props: {
  session: DecideResponse;
  file?: MediaFile;
  selection?: Selected;
  asked: Quality;
  /** Multi-variant sessions: the variant being played. */
  playing?: VariantOption;
  /** This device's bandwidth estimate (kbit/s). */
  estimate?: number;
  onClose: () => void;
}): JSX.Element {
  const d = () => props.session.decision;
  const q = () => props.session.quality;
  const adaptive = () => isAdaptive(props.session);
  return (
    <aside class="stream-info" aria-label="Stream info">
      <header>
        <p class="eyebrow eyebrow--accent">Stream info</p>
        <strong>{MODE_LABEL[props.session.mode] ?? props.session.mode}</strong>
        <p class="stream-info__why">{explainStream(props.session, props.asked)}</p>
      </header>
      <dl>
        <Show when={adaptive()}>
          <dt>Playing</dt>
          <dd>
            {props.playing
              ? `${props.playing.label} · ${props.playing.width}×${props.playing.height} · ${fmtMbps(props.playing.average_kbps)}${props.playing.transcode ? " · converted" : ""}`
              : "Starting…"}
          </dd>
          <dt>Versions</dt>
          <dd>{props.session.variants!.map((v) => v.label).join(" · ")}</dd>
        </Show>
        <Show when={props.estimate}>
          <dt>Connection</dt>
          <dd>~{fmtMbps(props.estimate)} (measured on this device)</dd>
        </Show>
        <Show when={q()}>
          <dt>{adaptive() ? "Best" : "Quality"}</dt>
          <dd>
            {QUALITY_NAME[q()!]}
            {props.session.bitrate_kbps ? ` · ${fmtMbps(props.session.bitrate_kbps)}` : ""}
            {props.asked !== q() && !adaptive() ? ` (asked for ${QUALITY_NAME[props.asked]})` : ""}
          </dd>
        </Show>
        <Show when={userLimitKbps()}>
          <dt>Limit</dt>
          <dd>{fmtMbps(userLimitKbps())} (your account)</dd>
        </Show>
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
      <Show when={props.selection?.reason}>
        <p class="stream-info__label">Track choice</p>
        <ul>
          <For each={props.selection!.reason.split(/;\s*/).filter(Boolean)}>{(r) => <li>{r.charAt(0).toUpperCase() + r.slice(1)}</li>}</For>
        </ul>
      </Show>
      <Show when={d().reasons.length}>
        <p class="stream-info__label">Server's reasons</p>
        <ul>
          <For each={d().reasons}>{(r) => <li>{r}</li>}</For>
        </ul>
      </Show>
      <p class="stream-info__hint"><span class="keycap">i</span> or <span class="keycap">Back</span> to close</p>
    </aside>
  );
}

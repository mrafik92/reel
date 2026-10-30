import { createEffect, createResource, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import {
  Icon, Sheet, audioLabel, createPlayerAdapter, episodeCode, focusElement, formatClock, iso1,
  AvPlayAdapter, mediaUrl, onBack, onRemoteKey, subtitleLabel,
  type ItemDetail, type MediaFile, type PlayerAdapter, type PlayerTracks,
} from "@reel/ui";
import { api, ApiError, invalidate, type DecideResponse, type SubtitleOption } from "../api";
import { casting, type CastPlayBody } from "../api/casting";
import { selectedOf, type Selected } from "../api/prefs";
import { creditItem, decide, MODE_LABEL } from "../lib/playback";
import { audioQuickSave, previewSelection, subtitleQuickSave, type QuickSave } from "../lib/tracks";
import { loadPrefs, playbackPrefs, prefsSupported, updatePrefs } from "../lib/prefs";
import {
  castFor, onCastEnd, startCast, stopCast, type CastOpen, type CastSource, type CastStream, type CastTarget, type CastTrack,
} from "../lib/casting";
import { CastCancelled } from "../lib/casting/chromecast";
import { TV, href } from "../lib/mode";
import { toast } from "../lib/toast";
import { ErrorState } from "../components/States";
import { CastSheet } from "../components/casting/CastSheet";
import { CastRemote } from "../components/casting/CastRemote";
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

export interface PlayError {
  title: string;
  body: string;
  retry: boolean;
  /** Extra fix-it action (e.g. "Stop the other stream"); playback restarts after it resolves. */
  action?: { label: string; run: () => Promise<void> };
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

export function PlayerView(props: { item: ItemDetail; source?: PlaySource }): JSX.Element {
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
  const [scrub, setScrub] = createSignal<number | null>(null);
  const [picker, setPicker] = createSignal<"audio" | "sub" | null>(null);
  const [series, setSeries] = createSignal<string>();
  const [session, setSession] = createSignal<DecideResponse>();
  const [info, setInfo] = createSignal(false);
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
      if (playing() && !picker() && scrub() == null && !info() && !quick() && !castSheet()) setVisible(false);
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

  /** Open a session (library decide or the page's source). */
  const openSession = (o: CastOpen, forCast?: "chromecast" | "airplay"): Promise<DecideResponse> =>
    props.source
      ? props.source.decide({ audio: o.audio, subtitle: o.subtitle ?? -1, subtitleIndex: o.subtitle, start: o.start, forCast })
      : decide(file!.id, { itemId: creditItem(file!, props.item.id), audio: o.audio, subtitle: o.subtitle, start: o.start, forCast });

  /** (Re)start playback: decide → load. Used on open and when audio/burn-in subs change. */
  const start = async (at: number, forCast?: "airplay") => {
    if (!file) return;
    setError(undefined);
    setBuffering(true);
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
      setBuffering(false);
      setError(props.source?.error?.(e) ?? decideError(e));
      return;
    }
    if (closed) {
      api.stopPlayback(d.session_id).catch(() => undefined);
      return;
    }
    setSession(d);
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
      a.on("cue", (c) => setCueHtml(c.text ? cueToHtml(c.text) : "")),
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
    if (!root || !bar) return;
    const top = bar.getBoundingClientRect().top - root.getBoundingClientRect().top;
    setSubLift(Math.max(0, root.clientHeight - top + root.clientHeight * 0.025));
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

  const chooseSub = (v: number) => {
    if (v === sub()) return;
    setSub(v);
    setSubAuto(false);
    subChosen = true;
    const p = playbackPrefs();
    if (p) {
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
    setVisible(true);
  };
  const resumeLocal = (at: number) => {
    localStopped = false;
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
    else navigate(props.source ? props.source.back : `/item/${props.item.id}`);
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

    tickTimer = setInterval(() => {
      if (playing()) report(false);
    }, TICK_MS);

    const onKey = (e: KeyboardEvent) => {
      if (picker() || castSheet()) return;
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
      else if (m === "play") adapter.play().catch(() => undefined);
      else if (m === "pause") adapter.pause();
      else if (m === "ff") seekBy(30);
      else if (m === "rw") seekBy(-10);
      else if (m === "stop") exit();
      else if (m === "info") setInfo((v) => !v);
      return true;
    });
    const offBack = onBack(() => {
      if (picker() || castSheet()) return false;
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
      props.source?.onClose?.();
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
  /** Session list (has VTT urls); falls back to the file's streams while (re)deciding. */
  const subList = (): { index: number; language?: string | null; title?: string | null; forced: boolean; external: boolean; url?: string | null }[] =>
    session()?.subtitles ?? file?.subtitles ?? [];
  const audioText = () => {
    const a = audioList().find((x) => x.index === audio());
    return a ? audioLabel(a).short : "Default";
  };
  const subText = () => {
    const s = subList().find((x) => x.index === sub());
    return s ? subLabel(s).short : "Off";
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
              <Show when={e().action}>
                {(a) => <ActionButton label={a().label} run={a().run} onDone={() => void start(pos() || resumeAt())} />}
              </Show>
              <Show when={e().retry}>
                <button type="button" class={e().action ? "btn" : "btn btn--primary"} data-autofocus={e().action ? undefined : ""} onClick={() => void start(pos() || resumeAt())}>
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
        {(s) => <StreamInfo session={s()} file={file} selection={selection()} onClose={() => setInfo(false)} />}
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

      <div class="player__chrome" data-nav-fixed aria-hidden={!visible() && playing() ? "true" : undefined}>
        <div class="player__top">
          <button type="button" class="iconbtn" aria-label="Back" onClick={exit}>
            <Icon name="back" />
          </button>
          <div class="player__titles">
            <Show when={props.item.kind === "episode" || props.source?.eyebrow} fallback={<h1>{props.item.title}</h1>}>
              <p class="eyebrow">{props.source?.eyebrow ?? `${series() ?? ""}${series() ? " · " : ""}${episodeCode(props.item)}`}</p>
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
              <button
                type="button"
                class="btn btn--ghost btn--select"
                onClick={() => setPicker("sub")}
                disabled={!subList().length}
                title={subAuto() ? "Picked automatically from your playback settings" : undefined}
              >
                <Icon name="cc" /> <span class="btn__value">{subText()}</span>
                <Show when={subAuto()}><small class="btn__tag">Auto</small></Show>
              </button>
              <Show when={!TV}>
                <button
                  type="button"
                  class="iconbtn"
                  classList={{ "is-casting": !!cast() }}
                  aria-label={cast() ? `Playing on ${cast()!.remote.device()}` : "Play on TV"}
                  title="Play on TV"
                  onClick={() => setCastSheet(true)}
                >
                  <Icon name={airplaying() ? "airplay" : "cast"} />
                </button>
              </Show>
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

/** Fix-it button on the error card: busy while it runs, then playback restarts. */
function ActionButton(props: { label: string; run: () => Promise<void>; onDone: () => void }): JSX.Element {
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
        <Icon name={busy() ? "clock" : "x"} /> <span>{busy() ? "Stopping…" : props.label}</span>
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
function StreamInfo(props: { session: DecideResponse; file?: MediaFile; selection?: Selected; onClose: () => void }): JSX.Element {
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
      <Show when={props.selection?.reason}>
        <p class="stream-info__label">Track choice</p>
        <ul>
          <For each={props.selection!.reason.split(/;\s*/).filter(Boolean)}>{(r) => <li>{r.charAt(0).toUpperCase() + r.slice(1)}</li>}</For>
        </ul>
      </Show>
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

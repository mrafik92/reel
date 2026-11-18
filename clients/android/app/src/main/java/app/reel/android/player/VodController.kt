package app.reel.android.player

import app.reel.android.data.DecideResponse
import app.reel.android.data.ProgressRequest
import app.reel.android.data.ReelError
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.plus

/** What the VOD player screen shows. */
data class VodState(
    val session: DecideResponse? = null,
    /** Opening a session (decide) or waiting for the first frame. */
    val loading: Boolean = true,
    /** Set while the player reopens a lost session by itself ("Reconnecting to the server"). */
    val recovering: String? = null,
    val error: PlayError? = null,
    /** ffprobe index of the audio track in use. */
    val audioIndex: Long? = null,
    /** ffprobe index of the subtitle in use; `-1` = off. */
    val subtitleIndex: Long = -1,
    /** The quality the viewer asked for (`auto` unless they picked one). */
    val quality: String = "auto",
    val playing: Boolean = false,
    val ended: Boolean = false,
)

/**
 * The VOD session state machine, on top of a [PlayerPort] and a [PlaySource] (web parity:
 * `Player.tsx`, #95, #100):
 *
 * - [start] decides, then loads the answer: HLS by mime type, sidecar WebVTT, the start position.
 *   A resume point within 5 s of the end starts from 0 instead.
 * - Progress goes out every [progressEveryMs] while playing (and as a keep-alive while paused, for
 *   up to 30 min), on pause, on the end and on [close], with the `paused` flag.
 * - A `404` on progress means the server reaped the session: decide again at the playhead (at
 *   once when playing, on the next play when the viewer paused).
 * - A fatal player error decides again at the playhead, once a minute at most; after that it shows
 *   an error with *Try again*.
 * - Audio / subtitle picks switch inside the loaded media when the player has the track (direct
 *   play, copied audio, sidecar or native subtitles); otherwise the server decides again.
 * - [close] reports the position and ends the session (`DELETE /playback/{sid}`).
 *
 * [scope] runs everything, including the final stop, so give it one that outlives the screen.
 * All calls must come from that scope's (main) thread.
 */
class VodController(
    private val source: PlaySource,
    private val player: PlayerPort,
    scope: CoroutineScope,
    /** Turns the server's relative URLs (`/api/v1/...`) into absolute ones. */
    private val resolve: (String) -> String,
    /** Monotonic clock in ms (tests pass the virtual clock). */
    private val now: () -> Long,
    /** The file's duration (s), when known before the player knows it: for the 5-s rule. */
    private val knownDurationS: Double? = null,
    private val progressEveryMs: Long = PROGRESS_EVERY_MS,
    initialQuality: String = "auto",
    private var startup: StartupTiming? = null,
    private val newTiming: () -> StartupTiming? = { null },
) : PlayerListener {
    private val outer = scope
    private val jobs = SupervisorJob(scope.coroutineContext[Job])
    private val cs = scope + jobs

    private val _state = MutableStateFlow(VodState(quality = initialQuality))
    val state: StateFlow<VodState> = _state.asStateFlow()

    private var openJob: Job? = null
    private var ticker: Job? = null
    private var closed = false

    /** The server no longer has the session (404 on progress). */
    private var gone = false

    /** The viewer paused (a pause the player causes while recovering isn't theirs). */
    private var held = false
    private var pausedAt = 0L
    private var lastAutoRestart: Long? = null

    /** False until the first answer: until then the server picks the subtitle. */
    private var subChosen = false
    private var startAt = 0.0
    private var checkedEnd = false

    init {
        player.listener = this
    }

    /** Playhead in seconds. */
    val positionS: Double get() = player.positionMs / 1000.0

    // ---- opening ----------------------------------------------------------------------------

    /** Open a session at [atS] (seconds, absolute) and play it. */
    fun start(atS: Double) {
        val dur = knownDurationS
        val at = if (atS > 0 && dur != null && dur > 0 && atS >= dur - END_SLACK_S) 0.0 else atS.coerceAtLeast(0.0)
        reopen(at, startup.also { startup = null })
    }

    /** *Try again* after an error. */
    fun retry() = reopen(currentOr(startAt))

    private fun currentOr(fallback: Double): Double = positionS.takeIf { it > 0 && _state.value.session != null } ?: fallback

    private var timing: StartupTiming? = null

    private fun reopen(at: Double, initial: StartupTiming? = null) {
        if (closed) return
        timing?.finish(StartupTiming.Outcome.superseded)
        val trace = initial ?: newTiming()
        timing = trace
        openJob?.cancel()
        gone = false
        startAt = at
        checkedEnd = false
        endSession()
        // The old stream is about to end on the server: drop it now, so its errors don't count.
        player.stop()
        _state.update { it.copy(error = null, loading = true, ended = false) }
        openJob = cs.launch {
            val s = _state.value
            trace?.mark(StartupTiming.Stage.decide)
            val d = try {
                source.open(
                    OpenRequest(
                        startS = at,
                        audioIndex = s.audioIndex,
                        subtitleIndex = if (subChosen) s.subtitleIndex else null,
                        quality = s.quality,
                    ),
                )
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                trace?.finish(StartupTiming.Outcome.failed)
                _state.update { it.copy(loading = false, recovering = null, error = source.errorFor(e)) }
                return@launch
            }
            if (closed) {
                outer.launch { stopQuietly(d.sessionId) }
                return@launch
            }
            trace?.decided(d.decision.mode.ifEmpty { d.mode })
            loaded(d, at, trace)
        }
    }

    private fun loaded(d: DecideResponse, at: Double, trace: StartupTiming?) {
        val firstAnswer = !subChosen
        subChosen = true
        _state.update { st ->
            st.copy(
                session = d,
                audioIndex = st.audioIndex ?: d.selected.audioIndex ?: d.decision.audioIndex
                    ?: d.audio.firstOrNull { it.default }?.index ?: d.audio.firstOrNull()?.index,
                subtitleIndex = if (firstAnswer) d.selected.subtitleIndex ?: -1 else st.subtitleIndex,
            )
        }
        player.load(
            PlayItem(
                url = resolve(d.url),
                startup = trace,
                mimeType = if (d.isHls) PlayItem.MIME_HLS else null,
                startMs = (at * 1000).toLong(),
                // Paused by the viewer (a reopened session, a track change): load, but stay paused (#100).
                autoplay = !held,
                subtitles = d.subtitles.mapNotNull { o ->
                    o.url?.let { SidecarText(sidecarId(o.index), resolve(it), o.language, subtitleLabel(o.language, o.title, o.forced)) }
                },
            ),
        )
        applyText()
        applyAudioLocally()
        ensureTicker()
    }

    // ---- player events ----------------------------------------------------------------------

    override fun onState(state: PlayerState) {
        when (state) {
            PlayerState.Ready -> _state.update { it.copy(loading = false) }
            PlayerState.Buffering -> _state.update { it.copy(loading = true) }
            PlayerState.Ended -> {
                _state.update { it.copy(loading = false, playing = false, ended = true) }
                report(paused = true)
            }
            PlayerState.Idle -> Unit
        }
    }

    override fun onIsPlaying(playing: Boolean) {
        _state.update { it.copy(playing = playing, recovering = if (playing) null else it.recovering) }
        if (playing) {
            held = false
            // Back from a long pause: make sure the server still has the session (a 404 reopens it).
            if (pausedAt != 0L && now() - pausedAt > RESUME_CHECK_MS) report(paused = false)
            pausedAt = 0
        } else if (!player.playWhenReady && _state.value.recovering == null) {
            // A pause (not buffering): the viewer's, or the system's (onStop, headphones out).
            held = true
            if (pausedAt == 0L) pausedAt = now()
            report(paused = true)
        }
    }

    override fun onDuration(durationMs: Long) {
        if (checkedEnd) return
        checkedEnd = true
        // Stale resume point past the real end (file replaced, bad progress): start over.
        if (startAt > 0 && startAt >= durationMs / 1000.0 - END_SLACK_S) {
            player.seekTo(0)
            if (!held) player.play()
        }
    }

    override fun onFatalError(message: String, decoder: Boolean) {
        if (closed) return
        val d = _state.value.session
        if (decoder && d != null && d.decision.video.type == "copy") {
            // The server sent the original picture and this device can't decode it (e.g. HEVC
            // Main 10 on a decoder without it). The same answer would fail again: offer a
            // conversion instead (a ladder rung always re-encodes to H.264).
            val rung = d.qualityOptions.firstOrNull { it.id != "original" && it.available }?.id ?: "720p"
            _state.update {
                it.copy(
                    loading = false,
                    recovering = null,
                    error = PlayError(
                        "This device can't play the original",
                        "Its decoder doesn't support this video (${d.decision.mode.ifEmpty { d.mode }.replace('_', ' ')}). Reel can convert it for you instead.",
                        retry = false,
                        action = PlayAction("Play transcoded") { _state.update { s -> s.copy(quality = rung) } },
                    ),
                )
            }
            return
        }
        val t = now()
        val last = lastAutoRestart
        // The player's own retries are spent. The server session may be gone too (reaped while
        // the stream was stuck): open a fresh one at the same spot, once a minute at most.
        if (last == null || t - last > AUTO_RESTART_EVERY_MS) {
            lastAutoRestart = t
            _state.update { it.copy(recovering = "Reconnecting to the server") }
            reopen(currentOr(startAt))
            return
        }
        _state.update {
            it.copy(
                loading = false,
                recovering = null,
                error = PlayError("Playback stopped", "${message.ifBlank { "The stream failed" }}. Try again, or pick a different quality."),
            )
        }
    }

    // ---- viewer actions ---------------------------------------------------------------------

    fun play() {
        held = false
        if (gone) {
            reopen(currentOr(startAt))
            return
        }
        if (_state.value.ended) player.seekTo(0)
        player.play()
    }

    fun pause() {
        held = true
        player.pause()
    }

    fun togglePlay() = if (player.playWhenReady && !_state.value.ended) pause() else play()

    /** Seek by [deltaS] seconds, clamped to the media. */
    fun seekBy(deltaS: Double) {
        val dur = player.durationMs
        var target = player.positionMs + (deltaS * 1000).toLong()
        if (dur != null) target = target.coerceAtMost(dur - 1000)
        seekTo(target.coerceAtLeast(0) / 1000.0)
    }

    fun seekTo(s: Double) {
        _state.update { it.copy(ended = false) }
        player.seekTo((s * 1000).toLong())
    }

    fun selectAudio(index: Long) {
        val st = _state.value
        if (index == st.audioIndex) return
        _state.update { it.copy(audioIndex = index) }
        val d = st.session ?: return
        if (canSwitchAudioLocally(d, index)) {
            player.selectAudio(d.audio.indexOfFirst { it.index == index })
            return
        }
        reopen(currentOr(startAt))
    }

    /** [index] `-1` turns subtitles off. */
    fun selectSubtitle(index: Long) {
        val st = _state.value
        if (index == st.subtitleIndex) return
        subChosen = true
        _state.update { it.copy(subtitleIndex = index) }
        val d = st.session ?: return
        val opt = d.subtitles.firstOrNull { it.index == index }
        val needsServer = when {
            // Leaving burn-in needs a clean stream.
            d.burnsSubtitles -> true
            index < 0 || opt == null -> false
            opt.url != null -> false
            // Picture subtitles: shown by the player itself when it has the track (direct play),
            // else the server burns them in.
            else -> !(d.isDirect && embeddedPosition(d, index)?.let(player::hasEmbeddedText) == true)
        }
        if (needsServer) reopen(currentOr(startAt)) else applyText()
    }

    fun selectQuality(quality: String) {
        if (quality == _state.value.quality && _state.value.error == null) return
        _state.update { it.copy(quality = quality) }
        reopen(currentOr(startAt))
    }

    /** Leave: report the position and end the session on the server. Safe to call twice. */
    fun close() {
        if (closed) return
        endSession()
        timing?.finish(StartupTiming.Outcome.canceled)
        closed = true
        jobs.cancel()
        player.listener = null
        player.stop()
    }

    // ---- internals --------------------------------------------------------------------------

    private fun applyText() {
        val st = _state.value
        val d = st.session ?: return
        val i = st.subtitleIndex
        val opt = d.subtitles.firstOrNull { it.index == i }
        val choice = when {
            i < 0 || opt == null || d.burnsSubtitles -> TextChoice.Off
            opt.url != null -> TextChoice.Sidecar(sidecarId(i))
            else -> embeddedPosition(d, i)?.let { TextChoice.Embedded(it) } ?: TextChoice.Off
        }
        player.selectText(choice)
    }

    private fun applyAudioLocally() {
        val st = _state.value
        val d = st.session ?: return
        val pos = d.audio.indexOfFirst { it.index == st.audioIndex }
        // Only for media that carries every audio track (direct play / copied audio).
        if (pos >= 0 && d.decision.audio.type == "copy" && d.mode in LOCAL_TRACK_MODES) player.selectAudio(pos)
    }

    private fun canSwitchAudioLocally(d: DecideResponse, index: Long): Boolean {
        val pos = d.audio.indexOfFirst { it.index == index }
        return pos >= 0 && d.decision.audio.type == "copy" && d.mode in LOCAL_TRACK_MODES &&
            player.audioTrackCount() == d.audio.size
    }

    /** Position of an embedded subtitle among the file's embedded subtitles (container order). */
    private fun embeddedPosition(d: DecideResponse, index: Long): Int? =
        d.subtitles.filter { !it.external }.sortedBy { it.index }.indexOfFirst { it.index == index }.takeIf { it >= 0 }

    private fun ensureTicker() {
        if (ticker?.isActive == true) return
        ticker = cs.launch {
            while (isActive) {
                delay(progressEveryMs)
                val paused = !player.playWhenReady
                if (!paused) {
                    report(paused = false)
                } else if (pausedAt != 0L && now() - pausedAt < PAUSED_KEEPALIVE_MS) {
                    // Paused: keep the session alive (a paused player with a full buffer requests nothing, #95).
                    report(paused = true)
                }
            }
        }
    }

    private fun report(paused: Boolean) {
        val d = _state.value.session ?: return
        if (closed) return
        val pos = positionS
        if (pos < 1) return
        outer.launch {
            try {
                source.progress(d.sessionId, ProgressRequest(pos, paused))
            } catch (e: CancellationException) {
                throw e
            } catch (e: ReelError.Http) {
                // 404: the server ended the session (idle reap, restart). Open a new one at the same spot.
                if (e.status == 404 && _state.value.session?.sessionId == d.sessionId) sessionGone()
            } catch (_: Exception) {
                // Offline for a moment: the next tick tries again.
            }
        }
    }

    private fun sessionGone() {
        if (gone || closed) return
        gone = true
        if (held) return
        _state.update { it.copy(recovering = "Reconnecting to the server") }
        reopen(currentOr(startAt))
    }

    /** Report the position, then stop the current session (if any), in that order. */
    private fun endSession() {
        val d = _state.value.session ?: return
        val pos = positionS
        _state.update { it.copy(session = null) }
        outer.launch {
            if (pos >= 1) {
                try {
                    source.progress(d.sessionId, ProgressRequest(pos, true))
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    // Gone already, or offline: stopping is what matters.
                }
            }
            stopQuietly(d.sessionId)
        }
    }

    private suspend fun stopQuietly(sessionId: String) {
        try {
            source.stop(sessionId)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Best effort: the server reaps idle sessions anyway.
        }
    }

    companion object {
        const val PROGRESS_EVERY_MS = 10_000L
        const val AUTO_RESTART_EVERY_MS = 60_000L
        const val RESUME_CHECK_MS = 45_000L
        const val PAUSED_KEEPALIVE_MS = 30 * 60_000L

        /** A resume point this close to the end (s) starts from the beginning. */
        const val END_SLACK_S = 5.0

        /** Modes whose media carries the file's own audio tracks. */
        val LOCAL_TRACK_MODES = setOf("direct_play", "remux")

        fun sidecarId(index: Long) = "reel-sub-$index"
    }
}

/** "English", "English · SDH", "English (forced)": short labels for menus and tracks. */
fun subtitleLabel(language: String?, title: String?, forced: Boolean): String {
    val lang = languageName(language)
    val base = when {
        lang != null && !title.isNullOrBlank() && !title.equals(lang, ignoreCase = true) -> "$lang · $title"
        lang != null -> lang
        !title.isNullOrBlank() -> title
        else -> "Unknown"
    }
    return if (forced) "$base (forced)" else base
}

/** ISO 639 code → display name (`eng` → English); unknown codes come back upper-cased. */
fun languageName(code: String?): String? {
    val c = code?.trim()?.lowercase()?.takeIf { it.isNotEmpty() && it != "und" } ?: return null
    val loc = java.util.Locale(c)
    val name = loc.getDisplayLanguage(java.util.Locale.ENGLISH)
    return if (name.isNullOrBlank() || name.equals(c, ignoreCase = true)) c.uppercase() else name
}

/** ISO 639 code → "EN" (two letters when there is one). */
fun shortLanguage(code: String?): String? {
    val c = code?.trim()?.lowercase()?.takeIf { it.isNotEmpty() && it != "und" } ?: return null
    if (c.length == 2) return c.uppercase()
    val two = java.util.Locale.getISOLanguages().firstOrNull { runCatching { java.util.Locale(it).isO3Language }.getOrNull() == c }
    return (two ?: c).uppercase()
}

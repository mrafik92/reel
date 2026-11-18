package app.reel.android.player

import app.reel.android.data.TuneResponse
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.plus
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Where live sessions come from: `POST /iptv/channels/{id}/tune` and `DELETE /iptv/sessions/{id}`. */
interface LiveSource {
    suspend fun tune(channelId: Long, takeover: Boolean): TuneResponse

    suspend fun stop(sessionId: String)

    /**
     * Viewer wording for a failed tune. [takeover] arms the next tune with `takeover: true` (the
     * admin 409's *Take over* button); the screen then tunes again.
     */
    fun errorFor(e: Throwable, takeover: () -> Unit): PlayError

    /** Remember a newly allocated session so a later busy answer can stop it. */
    fun remember(s: TuneResponse) {}
}

enum class LivePhase { Idle, Tuning, Playing, Error }

data class LiveState(
    /** The channel shown (set at once on a zap, before its tune answers). */
    val channelId: Long? = null,
    val session: TuneResponse? = null,
    val phase: LivePhase = LivePhase.Idle,
    val error: PlayError? = null,
    /** Set while the stream is being got back by itself (#93). */
    val recovering: String? = null,
)

/**
 * The live session state machine on a [PlayerPort] (web parity: `components/live/tuner.ts`):
 *
 * - [tune] waits for the current session to be released (`DELETE`) and then
 *   tunes `passthrough`; ExoPlayer plays the signed `stream.ts`. Answers to an older tune are
 *   stopped and dropped.
 * - [zap] shows the new channel at once but tunes only once the viewer stops pressing
 *   ([zapSettleMs]), so flicking through ten channels doesn't open ten provider streams.
 * - A fatal player error re-tunes by itself once a minute at most (#93); after that it shows an
 *   error with *Try again*.
 * - [background] frees the provider slot when the app leaves the screen and [foreground] tunes
 *   the same channel again; [close] frees it for good.
 *
 * [scope] runs everything, including the final stop, so give it one that outlives the screen.
 * All calls must come from that scope's (main) thread.
 */
class LiveController(
    private val source: LiveSource,
    private val player: PlayerPort,
    scope: CoroutineScope,
    /** Turns the server's relative URLs (`/api/v1/...`) into absolute ones. */
    private val resolve: (String) -> String,
    /** Monotonic clock in ms (tests pass the virtual clock). */
    private val now: () -> Long,
    private val zapSettleMs: Long = ZAP_SETTLE_MS,
    private val sessionLock: Mutex = Mutex(),
) : PlayerListener {
    private val outer = scope
    private val jobs = SupervisorJob(scope.coroutineContext[Job])
    private val cs = scope + jobs

    private val _state = MutableStateFlow(LiveState())
    val state: StateFlow<LiveState> = _state.asStateFlow()

    /** Bumped by every tune / stop: answers carrying an older value are stale. */
    private var gen = 0
    private var tuneJob: Job? = null
    private var takeoverNext = false
    private var lastAuto: Long? = null
    private var closed = false

    /** Playing (or tuning) when the app went to the background. */
    private var resumeOnForeground = false

    init {
        player.listener = this
    }

    /** Tune [channelId] now. */
    fun tune(channelId: Long) = start(channelId, delayMs = 0, auto = false)

    /** Switch to [channelId]: shown at once, tuned once the viewer settles on it. */
    fun zap(channelId: Long) = start(channelId, delayMs = zapSettleMs, auto = false)

    /** *Try again* after an error (with `takeover` once if *Take over* was pressed). */
    fun retry() {
        _state.value.channelId?.let { start(it, delayMs = 0, auto = false) }
    }

    /** The admin 409's *Take over*: the next tune asks the server to stop the other streams. */
    fun armTakeover() {
        takeoverNext = true
    }

    private fun start(channelId: Long, delayMs: Long, auto: Boolean) {
        if (closed) return
        val my = ++gen
        tuneJob?.cancel()
        _state.update {
            it.copy(
                channelId = channelId,
                phase = LivePhase.Tuning,
                error = null,
                recovering = if (auto) it.recovering else null,
            )
        }
        tuneJob = cs.launch {
            if (delayMs > 0) delay(delayMs)
            // Release our slot before asking for a new one (the provider allows one connection).
            release()
            sessionLock.withLock {
                if (my != gen) return@withLock
                val takeover = takeoverNext
                takeoverNext = false
                // A cancelled POST may already have allocated a slot. Read its answer and
                // delete a stale session before the next caller can enter the lock.
                withContext(NonCancellable) { finishTune(channelId, takeover, my) }
            }
        }
    }

    private suspend fun finishTune(channelId: Long, takeover: Boolean, generation: Int) {
        val session = try {
            source.tune(channelId, takeover)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (generation == gen) {
                _state.update { it.copy(phase = LivePhase.Error, recovering = null, error = source.errorFor(e, ::armTakeover)) }
            }
            return
        }
        source.remember(session)
        if (generation != gen || closed) {
            stopQuietly(session.sessionId)
            return
        }
        _state.update { it.copy(session = session) }
        player.load(PlayItem(url = resolve(session.url), mimeType = PlayItem.MIME_TS, title = session.channelName))
    }

    private fun release() {
        val session = _state.value.session ?: return
        _state.update { it.copy(session = null) }
        player.stop()
        outer.launch(start = CoroutineStart.UNDISPATCHED) {
            sessionLock.withLock { stopQuietly(session.sessionId) }
        }
    }

    private suspend fun stopQuietly(sessionId: String) {
        try {
            source.stop(sessionId)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Best effort: passthrough sessions also end when the player disconnects.
        }
    }

    /** Stop watching (the channel stays selected, so [retry] / [foreground] can tune it again). */
    fun stop() {
        gen++
        tuneJob?.cancel()
        release()
        player.stop()
        _state.update { it.copy(phase = LivePhase.Idle, recovering = null, error = null) }
    }

    /** The app went to the background: free the provider slot, and remember to come back. */
    fun background() {
        val p = _state.value.phase
        if (p == LivePhase.Playing || p == LivePhase.Tuning) {
            resumeOnForeground = true
            stop()
        }
    }

    /** Back to the front: tune the channel that was playing. */
    fun foreground() {
        if (!resumeOnForeground) return
        resumeOnForeground = false
        retry()
    }

    /** The screen is gone: stop for good. */
    fun close() {
        if (closed) return
        stop()
        closed = true
        jobs.cancel()
        player.listener = null
    }

    // ---- player events ----------------------------------------------------------------------

    override fun onState(state: PlayerState) {
        when (state) {
            PlayerState.Ready -> _state.update {
                if (it.session?.channelId == it.channelId && it.session != null && it.phase == LivePhase.Tuning) it.copy(phase = LivePhase.Playing, recovering = null) else it
            }
            // A live stream that "ends" lost its source: treat it like a failure.
            PlayerState.Ended -> if (_state.value.session != null) onFatalError("The channel stopped sending", decoder = false)
            else -> Unit
        }
    }

    override fun onIsPlaying(playing: Boolean) {
        if (playing) onState(PlayerState.Ready)
    }

    override fun onFatalError(message: String, decoder: Boolean) {
        val ch = _state.value.channelId ?: return
        if (_state.value.session == null) return
        val t = now()
        val last = lastAuto
        if (!decoder && (last == null || t - last >= AUTO_RETUNE_EVERY_MS)) {
            lastAuto = t
            _state.update { it.copy(recovering = "Reconnecting to the channel…") }
            start(ch, delayMs = 0, auto = true)
            return
        }
        gen++
        tuneJob?.cancel()
        release()
        _state.update {
            it.copy(
                phase = LivePhase.Error,
                recovering = null,
                error = PlayError(
                    if (decoder) "This device can't play this channel" else "The channel stopped",
                    if (decoder) "$message. Try another channel." else "$message. Try again, or pick another channel.",
                    retry = true,
                ),
            )
        }
    }

    companion object {
        /** Zaps closer together than this tune only the last channel. */
        const val ZAP_SETTLE_MS = 350L

        /** At most one automatic re-tune a minute (#93). */
        const val AUTO_RETUNE_EVERY_MS = 60_000L
    }
}

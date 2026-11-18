package app.reel.android.player

import android.os.SystemClock
import android.util.Log
import app.reel.android.BuildConfig
import java.util.concurrent.atomic.AtomicLong

/** Main-thread, one-shot trace. Never accepts URLs, exceptions, or item/session identifiers. */
class StartupTiming internal constructor(
    val id: Long,
    private val startedMs: Long,
    private val vod: Boolean,
    private val now: () -> Long,
    private val emit: (String) -> Unit,
) {
    private val marks = mutableMapOf<Stage, Long>()
    private var finished = false
    private var mode = "unknown"

    enum class Stage { route, decide, decided, load, response, videoFormat, decoder, frame }
    enum class Outcome { rendered, failed, canceled, superseded }

    fun mark(stage: Stage, at: Long = now()) {
        if (!finished && at >= startedMs) marks.putIfAbsent(stage, at - startedMs)
    }

    fun decided(value: String) {
        mode = when (value) {
            "direct_play", "remux", "audio_transcode", "transcode", "passthrough" -> value
            else -> "unknown"
        }
        mark(Stage.decided)
    }

    fun finish(outcome: Outcome) {
        if (finished) return
        finished = true
        val stages = Stage.entries.joinToString("") { stage ->
            marks[stage]?.let { " ${stage.name}_ms=$it" }.orEmpty()
        }
        emit("attempt=$id outcome=${outcome.name} source=${if (vod) "vod" else "library"} mode=$mode$stages")
    }

    companion object {
        private val ids = AtomicLong()

        fun tap(): Long = if (BuildConfig.DEBUG) SystemClock.elapsedRealtime() else 0

        fun begin(vod: Boolean, tapMs: Long = 0): StartupTiming? {
            if (!BuildConfig.DEBUG) return null
            val now = tap()
            return StartupTiming(ids.incrementAndGet(), tapMs.takeIf { it in 1..now } ?: now, vod, ::tap) {
                Log.i("ReelStartup", it)
            }
        }
    }
}

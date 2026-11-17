package app.reel.android.player

import app.reel.android.data.DecideRequest
import app.reel.android.data.DecideResponse
import app.reel.android.data.ProgressRequest
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError

/** What the VOD player asks a source for when it (re)opens a session. */
data class OpenRequest(
    /** Absolute start position (s). */
    val startS: Double,
    /** ffprobe index; `null` = the server picks from the user's preferences. */
    val audioIndex: Long? = null,
    /** ffprobe index, `-1` = none, `null` = the server picks. */
    val subtitleIndex: Long? = null,
    /** `auto` | `original` | `1080p` | `720p` | `480p`. */
    val quality: String = "auto",
)

/** Why playback can't go on, worded for the viewer. [action] is an extra button (e.g. *Take over*). */
data class PlayError(
    val title: String,
    val body: String,
    val retry: Boolean = true,
    val action: PlayAction? = null,
)

data class PlayAction(val label: String, val run: suspend () -> Unit)

/**
 * Where the VOD player's sessions come from. The library uses `POST /playback/{file}/decide`; IPTV
 * VOD (#193) uses `POST /vod/play`, which answers the same way. Progress and stop are the same
 * endpoints for both.
 */
interface PlaySource {
    suspend fun open(o: OpenRequest): DecideResponse

    suspend fun progress(sessionId: String, body: ProgressRequest)

    suspend fun stop(sessionId: String)

    /** Viewer-facing wording for a failed [open]. */
    fun errorFor(e: Throwable): PlayError = defaultPlayError(e)
}

/** Progress and stop through `/playback/{sid}`; subclasses only say how to open. */
abstract class ReelPlaySource(protected val api: ReelApi) : PlaySource {
    override suspend fun progress(sessionId: String, body: ProgressRequest) {
        api.post<ProgressRequest, Unit>("/playback/$sessionId/progress", body)
    }

    override suspend fun stop(sessionId: String) {
        api.delete("/playback/$sessionId")
    }
}

/** A library file: `POST /playback/{file}/decide` with the ExoPlayer preset and this install's device id. */
class LibrarySource(
    api: ReelApi,
    private val fileId: Long,
    private val itemId: Long?,
    private val deviceId: String?,
) : ReelPlaySource(api) {
    override suspend fun open(o: OpenRequest): DecideResponse =
        api.post<DecideRequest, DecideResponse>("/playback/$fileId/decide", decideRequest(o, deviceId, itemId))

    override fun errorFor(e: Throwable): PlayError =
        if (e is ReelError.Http && e.status == 404) {
            PlayError("File not found", "This file is no longer on the server. A rescan may fix it.", retry = false)
        } else {
            defaultPlayError(e)
        }

    companion object {
        fun decideRequest(o: OpenRequest, deviceId: String?, itemId: Long?) = DecideRequest(
            deviceId = deviceId,
            itemId = itemId?.takeIf { it > 0 },
            startS = o.startS.takeIf { it > 0 },
            audioIndex = o.audioIndex,
            subtitleIndex = o.subtitleIndex,
            quality = o.quality,
        )
    }
}

fun defaultPlayError(e: Throwable): PlayError = when (e) {
    is ReelError.RateLimited -> PlayError("Reel is busy", e.userMessage)
    is ReelError -> PlayError("Couldn't start playback", e.userMessage)
    else -> PlayError("Couldn't start playback", e.message ?: "Something went wrong")
}

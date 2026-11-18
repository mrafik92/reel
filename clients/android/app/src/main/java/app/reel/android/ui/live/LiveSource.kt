package app.reel.android.ui.live

import app.reel.android.data.Me
import app.reel.android.data.OwnLiveSession
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.TuneResponse
import app.reel.android.data.stopLive
import app.reel.android.data.tune
import app.reel.android.player.LiveSource
import app.reel.android.player.PlayAction
import app.reel.android.player.PlayError
import app.reel.android.ui.vod.vodPlayError

/**
 * Live TV on the Reel API. [own] is this device's last session (from `DevicePrefs`), kept current
 * through [save] so a 429 can offer *Stop the other stream* after a crash left it running.
 */
class ApiLiveSource(
    private val api: ReelApi,
    private val me: Me?,
    private var own: OwnLiveSession?,
    private val save: (OwnLiveSession?) -> Unit,
) : LiveSource {
    override suspend fun tune(channelId: Long, takeover: Boolean): TuneResponse = api.tune(channelId, takeover)

    override suspend fun stop(sessionId: String) {
        try {
            api.stopLive(sessionId)
        } catch (e: ReelError.Http) {
            if (e.status != 404) throw e
        }
        if (own?.id == sessionId) {
            own = null
            save(null)
        }
    }

    override fun remember(s: TuneResponse) {
        own = OwnLiveSession(s.sessionId, s.channelName)
        save(own)
    }

    override fun errorFor(e: Throwable, takeover: () -> Unit): PlayError =
        liveTuneError(e, me, own, takeover, stopOwn = ::stop, stopVod = { id ->
            api.delete("/playback/$id")
        })
}

/**
 * Viewer wording for a failed tune (web parity: `friendlyTuneError`):
 * - 429 while this device still has a session of its own → **Stop the other stream**
 * - admin 409 `{ takeover: true }` → names who's watching, **Take over** (tunes once with `takeover`)
 * - other busy answers: who's watching, when the server says (the same wording as IPTV VOD)
 * - 503: off air; 404: the channel is gone.
 */
fun liveTuneError(
    e: Throwable,
    me: Me?,
    own: OwnLiveSession?,
    takeover: () -> Unit,
    stopOwn: suspend (String) -> Unit,
    stopVod: suspend (String) -> Unit,
): PlayError {
    val status = (e as? ReelError.Http)?.status ?: if (e is ReelError.RateLimited) 429 else 0
    if (status == 429 && own != null) {
        val ch = own.channel.ifEmpty { "another channel" }
        return PlayError(
            "You're already watching on this device",
            "An earlier stream of $ch from this device may still hold your provider's only connection. " +
                "Stop it and this channel starts right away.",
            retry = true,
            action = PlayAction("Stop the other stream") { stopOwn(own.id) },
        )
    }
    return when (status) {
        429, 409 -> vodPlayError(e, me, takeover, stopHolder = { h ->
            h.sessionId?.let { if (h.kind == "vod") stopVod(it) else stopOwn(it) }
        }).let {
            if (it.title == "Couldn't start this title") it.copy(title = "Couldn't start this channel") else it
        }
        503 -> PlayError(
            "This channel isn't sending any picture",
            "The provider didn't deliver a stream. It may be off air for a moment, so try again or pick another channel.",
        )
        404 -> PlayError("This channel is no longer available", "It was removed from your provider's list. Pick another channel.", retry = false)
        else -> when (e) {
            is ReelError.Offline, is ReelError.Unreachable, is ReelError.Timeout -> PlayError("Can't reach Reel", "Check your connection and try again.")
            is ReelError -> PlayError("Couldn't start this channel", e.userMessage)
            else -> PlayError("Couldn't start this channel", e.message ?: "Something went wrong")
        }
    }
}

package app.reel.android.ui.vod

import app.reel.android.data.DecideResponse
import app.reel.android.data.Me
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.ReelJson
import app.reel.android.data.VodPlayRequest
import app.reel.android.player.OpenRequest
import app.reel.android.player.PlayAction
import app.reel.android.player.PlayError
import app.reel.android.player.ReelPlaySource
import kotlin.math.floor
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.longOrNull

/**
 * An IPTV movie or episode for the VOD player: `POST /vod/play` instead of decide (same answer);
 * progress and stop use the usual `/playback/{sid}` endpoints. Busy answers are worded like the
 * web (`components/vod/busy.ts`).
 */
class VodSource(
    api: ReelApi,
    /** `movie` | `episode`. */
    private val kind: String,
    private val id: Long,
    private val deviceId: String?,
    private val me: Me?,
) : ReelPlaySource(api) {
    /** Set by *Take over* (admins): the next open asks the server to stop the other streams. */
    var takeover = false

    override suspend fun open(o: OpenRequest): DecideResponse {
        val t = takeover
        takeover = false
        return api.post<VodPlayRequest, DecideResponse>("/vod/play", request(o, t))
    }

    fun request(o: OpenRequest, takeover: Boolean) = VodPlayRequest(
        kind = kind,
        id = id,
        deviceId = deviceId,
        startS = o.startS.takeIf { it > 0 }?.let { floor(it) },
        audioIndex = o.audioIndex,
        subtitleIndex = o.subtitleIndex?.takeIf { it >= 0 },
        quality = o.quality,
        takeover = if (takeover) true else null,
    )

    override fun errorFor(e: Throwable): PlayError = vodPlayError(
        e,
        me,
        takeover = { takeover = true },
        stopHolder = { h -> h.sessionId?.let { sid -> api.delete(if (h.kind == "live") "/iptv/sessions/$sid" else "/playback/$sid") } },
    )
}

/** Who holds the provider connection (from a busy answer). */
data class VodHolder(
    val userId: Long? = null,
    val username: String? = null,
    val mine: Boolean? = null,
    val sessionId: String? = null,
    /** `live` | `vod`. */
    val kind: String? = null,
    val title: String? = null,
    val device: String? = null,
)

/**
 * Holders from a busy body. Two shapes: `{ holder }` (VOD) or the Live TV budget's admin 409
 * `{ takeover: true, holders: [{ user_id, username, session_id, channel_name, … }] }`.
 */
fun holdersOf(body: String?): List<VodHolder> {
    val o = body?.let { runCatching { ReelJson.parseToJsonElement(it).jsonObject }.getOrNull() } ?: return emptyList()
    fun str(j: JsonObject, k: String) = (j[k] as? JsonPrimitive)?.contentOrNull
    fun holder(j: JsonObject): VodHolder {
        val channel = str(j, "channel_name")
        return VodHolder(
            userId = (j["user_id"] as? JsonPrimitive)?.longOrNull,
            username = str(j, "username"),
            mine = (j["mine"] as? JsonPrimitive)?.booleanOrNull,
            sessionId = str(j, "session_id"),
            kind = str(j, "kind") ?: if (channel != null) "live" else null,
            title = str(j, "title") ?: channel,
            device = str(j, "device"),
        )
    }
    (o["holder"] as? JsonObject)?.let { return listOf(holder(it)) }
    return (o["holders"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let(::holder) }.orEmpty()
}

private fun bodyOf(e: ReelError): String? = when (e) {
    is ReelError.Http -> e.body
    is ReelError.RateLimited -> e.body
    else -> null
}

private fun describe(h: VodHolder): String {
    val what = h.title?.let { (if (h.kind == "live") "the channel " else "") + it } ?: "something"
    return what + (h.device?.let { " on $it" } ?: "")
}

private fun isMine(h: VodHolder?, msg: String, me: Me?): Boolean {
    h?.mine?.let { return it }
    if (h != null && me != null && (h.userId == me.id || (h.username != null && h.username == me.username))) return true
    return h == null && Regex("""\byou(?:'re| are)\b|\byour other\b""", RegexOption.IGNORE_CASE).containsMatchIn(msg)
}

private fun String.sentence() = if (endsWith(".")) "$this " else "$this. "

/**
 * Viewer wording for IPTV playback failures. Providers usually allow one connection, so "busy" is
 * the common case:
 * - the viewer's own other screen → **Stop the other stream**
 * - admin 409 `{ takeover: true }` → name who's watching, **Take over**
 * - anyone else → a plain explanation (non-admins aren't told who)
 */
fun vodPlayError(e: Throwable, me: Me?, takeover: () -> Unit, stopHolder: suspend (VodHolder) -> Unit): PlayError {
    if (e !is ReelError) return PlayError("Couldn't start this title", e.message ?: "Something went wrong")
    val status = when (e) {
        is ReelError.Http -> e.status
        is ReelError.RateLimited -> 429
        else -> 0
    }
    val body = bodyOf(e)
    val takeoverOffered = status == 409 && body?.let {
        runCatching { (ReelJson.parseToJsonElement(it).jsonObject["takeover"] as? JsonPrimitive)?.booleanOrNull }.getOrNull()
    } == true
    if (status == 429 || takeoverOffered) {
        val hs = holdersOf(body)
        val h = hs.firstOrNull()
        val msg = e.message?.takeIf { it.isNotBlank() && it != "busy" && it != ReelError.friendly(429) }.orEmpty()
        if (isMine(h, msg, me)) {
            val canStop = h?.sessionId != null
            return PlayError(
                "You're already watching IPTV on another screen",
                (if (h != null) "You're watching ${describe(h)}. " else if (msg.isNotEmpty()) msg.sentence() else "") +
                    "Your provider only allows one stream at a time" +
                    (if (canStop) ", so stop that one and this starts right away." else ". Stop it there, then try again."),
                retry = true,
                action = if (canStop) PlayAction("Stop the other stream") { stopHolder(h!!) } else null,
            )
        }
        if (takeoverOffered) {
            val who = h?.username ?: "Someone else"
            val others = if (hs.size > 1) " (and ${hs.size - 1} more)" else ""
            val n = hs.size.coerceAtLeast(1)
            return PlayError(
                "Your IPTV connection is in use",
                "$who$others is watching ${h?.let(::describe) ?: "something"}. Your provider only allows $n stream${if (n > 1) "s" else ""} " +
                    "at a time. Take over to stop theirs and start this.",
                retry = true,
                action = PlayAction("Take over") { takeover() },
            )
        }
        // The server's anonymous wording ("All 1 connection(s) … in use") reads like a log line.
        val named = if (msg.isNotEmpty() && !Regex("""connection\(s\)|in use by other viewers""", RegexOption.IGNORE_CASE).containsMatchIn(msg)) msg.sentence() else ""
        val wait = (e as? ReelError.RateLimited)?.retryAfterS?.let { " Or try again in ${((it + 59) / 60).coerceAtLeast(1)} min." } ?: ""
        return PlayError(
            "Your IPTV connection is in use",
            (if (h?.username != null) "${h.username} is watching ${describe(h)}. " else named.ifEmpty { "Someone else in your household is watching IPTV right now. " }) +
                "Your provider only allows one stream at a time, so this can start once they stop." + wait,
            retry = true,
        )
    }
    return when {
        status == 404 -> PlayError("No longer on your provider", "Your IPTV provider removed this title. It may come back after the next catalogue refresh.", retry = false)
        status in 502..504 -> PlayError("Your IPTV provider didn't answer", "${(e.message ?: "No answer").sentence()}Providers drop out now and then, so try again in a moment.")
        e is ReelError.Offline || e is ReelError.Unreachable || e is ReelError.Timeout -> PlayError("Can't reach Reel", "Check your connection and try again.")
        else -> PlayError("Couldn't start this title", e.userMessage)
    }
}

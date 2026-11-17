package app.reel.android.data

import kotlinx.serialization.serializer

/** Account endpoints used by Settings (#194): playback preferences and signed-in devices. */

suspend inline fun <reified B, reified T> ReelApi.put(path: String, body: B): T =
    request("PUT", path, emptyMap(), encode(serializer<B>(), body), serializer<T>(), true)

suspend fun ReelApi.preferences(): PlaybackPrefs = get("/me/preferences")

/** Saves the whole set and returns what the server stored. */
suspend fun ReelApi.savePreferences(p: PlaybackPrefs): PlaybackPrefs = put("/me/preferences", p)

suspend fun ReelApi.sessions(): List<SessionInfo> = get("/me/sessions")

/** Signs that device out (its token stops working). */
suspend fun ReelApi.revokeSession(id: Long) = delete("/me/sessions/$id")

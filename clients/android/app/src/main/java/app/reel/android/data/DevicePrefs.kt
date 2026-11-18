package app.reel.android.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.longPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

/**
 * Settings that belong to this device, not the account (#194), like the web's per-device quality:
 * the right choice depends on the screen and the network it's on.
 */
class DevicePrefs(private val store: DataStore<Preferences>) {
    /** Starting quality for films and episodes: one of [QUALITIES] (`auto` by default). */
    val quality: Flow<String> = store.data.map { p -> p[QUALITY]?.takeIf { it in QUALITIES } ?: "auto" }

    suspend fun setQuality(q: String) {
        require(q in QUALITIES) { "unknown quality $q" }
        store.edit { it[QUALITY] = q }
    }

    /** The channel Live TV played last (#191): where it starts next time. */
    val lastChannel: Flow<Long?> = store.data.map { it[LAST_CHANNEL] }

    suspend fun setLastChannel(id: Long) {
        store.edit { it[LAST_CHANNEL] = id }
    }

    /**
     * This device's live session that may still hold the provider connection (set on tune, cleared
     * on stop). After a crash it lets a busy answer offer *Stop the other stream* (web parity).
     */
    fun liveSession(owner: String): Flow<OwnLiveSession?> = store.data.map { p ->
        p[LIVE_SESSION]?.takeIf { p[LIVE_OWNER] == owner }?.let { OwnLiveSession(it, p[LIVE_CHANNEL].orEmpty()) }
    }

    suspend fun setLiveSession(owner: String, s: OwnLiveSession?) {
        store.edit {
            if (s == null) {
                it.remove(LIVE_SESSION)
                it.remove(LIVE_CHANNEL)
                it.remove(LIVE_OWNER)
            } else {
                it[LIVE_SESSION] = s.id
                it[LIVE_CHANNEL] = s.channel
                it[LIVE_OWNER] = owner
            }
        }
    }

    companion object {
        /** The values `decide` takes for `quality` (server `Quality`). */
        val QUALITIES = listOf("auto", "original", "1080p", "720p", "480p")
        private val QUALITY = stringPreferencesKey("quality")
        private val LAST_CHANNEL = longPreferencesKey("live_last_channel")
        private val LIVE_SESSION = stringPreferencesKey("live_session")
        private val LIVE_OWNER = stringPreferencesKey("live_session_owner")
        private val LIVE_CHANNEL = stringPreferencesKey("live_session_channel")
    }
}

/** A live session this device started: its id and the channel's name. */
data class OwnLiveSession(val id: String, val channel: String)

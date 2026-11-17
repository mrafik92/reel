package app.reel.android.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
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

    companion object {
        /** The values `decide` takes for `quality` (server `Quality`). */
        val QUALITIES = listOf("auto", "original", "1080p", "720p", "480p")
        private val QUALITY = stringPreferencesKey("quality")
    }
}

package app.reel.android.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import java.util.UUID
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

/** What the app remembers between launches. */
data class StoredSession(
    val server: ServerAddress? = null,
    val token: String? = null,
    /** The signed-in user as last seen, so the app opens offline too. */
    val user: Me? = null,
)

/**
 * Server address, session token, cached user and the per-install device id, in app-private
 * DataStore. The manifest turns backups and device transfer off, so the token never leaves the
 * device.
 */
class SessionStore(private val store: DataStore<Preferences>) {
    val session: Flow<StoredSession> = store.data.map { p ->
        StoredSession(
            server = p[SERVER]?.let(ServerAddress::parse),
            token = p[TOKEN],
            user = p[USER]?.let { runCatching { ReelJson.decodeFromString(Me.serializer(), it) }.getOrNull() },
        )
    }

    suspend fun current(): StoredSession = session.first()

    /**
     * A random id made once per install. The server keys sessions on it (signing in again replaces
     * this install's session) and its "newest wins" playback rules tell devices apart with it.
     */
    suspend fun deviceId(): String {
        store.data.first()[DEVICE_ID]?.let { return it }
        var id = ""
        store.edit { p -> id = p[DEVICE_ID] ?: UUID.randomUUID().toString().also { p[DEVICE_ID] = it } }
        return id
    }

    /** A different server signs the user out (the token belongs to the old one). */
    suspend fun setServer(server: ServerAddress) {
        store.edit { p ->
            if (p[SERVER] != server.origin) {
                p.remove(TOKEN)
                p.remove(USER)
            }
            p[SERVER] = server.origin
        }
    }

    suspend fun signedIn(token: String, user: Me) {
        store.edit { p ->
            p[TOKEN] = token
            p[USER] = ReelJson.encodeToString(Me.serializer(), user)
        }
    }

    suspend fun updateUser(user: Me) {
        store.edit { p -> if (p[TOKEN] != null) p[USER] = ReelJson.encodeToString(Me.serializer(), user) }
    }

    /** Sign out locally: forget the token and user, keep the server and device id. */
    suspend fun clearToken() {
        store.edit { p ->
            p.remove(TOKEN)
            p.remove(USER)
        }
    }

    private companion object {
        val SERVER = stringPreferencesKey("server")
        val TOKEN = stringPreferencesKey("token")
        val USER = stringPreferencesKey("user")
        val DEVICE_ID = stringPreferencesKey("device_id")
    }
}

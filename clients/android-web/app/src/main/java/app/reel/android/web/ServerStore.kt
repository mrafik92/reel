package app.reel.android.web

import android.content.Context

/**
 * The configured server, in app-private SharedPreferences (excluded from backups). The web session
 * itself lives in the WebView's localStorage for that origin, so it survives restarts, and switching
 * between a LAN and a Tailscale address keeps each origin's session.
 */
class ServerStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("reel_web", Context.MODE_PRIVATE)

    var server: ServerAddress?
        get() = prefs.getString(KEY, null)?.let(ServerAddress::parse)
            ?: BuildConfig.DEFAULT_SERVER.takeIf { it.isNotEmpty() }?.let(ServerAddress::parse)
        set(value) {
            prefs.edit().apply { if (value == null) remove(KEY) else putString(KEY, value.origin) }.apply()
        }

    private companion object {
        const val KEY = "server"
    }
}

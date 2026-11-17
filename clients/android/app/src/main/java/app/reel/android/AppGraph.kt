package app.reel.android

import android.content.Context
import android.net.ConnectivityManager
import android.os.Build
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.preferencesDataStoreFile
import app.reel.android.data.DevicePrefs
import app.reel.android.data.SessionManager
import app.reel.android.data.SessionStore
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import okhttp3.OkHttpClient

/**
 * Process-wide objects, made once in [ReelApplication]. Screens reach them through [LocalAppGraph];
 * data calls go through `graph.session.api` (the configured server's [app.reel.android.data.ReelApi]).
 */
class AppGraph(context: Context) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** Shared connection pool. [app.reel.android.data.ReelApi] adds the token interceptor on top. */
    val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .build()

    private val connectivity = context.getSystemService(ConnectivityManager::class.java)

    /** Per-device settings (playback quality); the player reads them (#190). */
    val devicePrefs = DevicePrefs(PreferenceDataStoreFactory.create { context.preferencesDataStoreFile("device_prefs") })

    val session = SessionManager(
        store = SessionStore(PreferenceDataStoreFactory.create { context.preferencesDataStoreFile("session") }),
        http = http,
        scope = scope,
        deviceName = deviceName(),
        defaultServer = BuildConfig.DEFAULT_SERVER,
        isOnline = { connectivity?.activeNetwork != null },
    )

    companion object {
        /** "Google Pixel 7", "NVIDIA SHIELD Android TV": shown in the web's *Signed-in devices*. */
        fun deviceName(manufacturer: String = Build.MANUFACTURER, model: String = Build.MODEL): String {
            val maker = manufacturer.replaceFirstChar { it.uppercase() }
            return if (model.startsWith(manufacturer, ignoreCase = true)) model else "$maker $model".trim()
        }
    }
}

val LocalAppGraph = staticCompositionLocalOf<AppGraph> { error("AppGraph not provided") }

package app.reel.android.web

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper

/**
 * Watches the default network while the activity is started (#228), so the error panel can retry when
 * connectivity returns. [onChange] runs on the main thread with `newNetwork = true` when a network
 * becomes the default or gains internet validation (Wi-Fi joining, airplane mode off, Tailscale
 * connecting), and `false` when the phone loses its last network.
 *
 * A LAN without internet never validates but still counts as online: a private Reel server doesn't
 * need the internet.
 */
class NetworkWatch(context: Context, private val onChange: (newNetwork: Boolean) -> Unit) {
    private val cm = context.getSystemService(ConnectivityManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var registered = false
    private var validated = false

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            validated = false
            main.post { onChange(true) }
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            val now = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
            if (now && !validated) main.post { onChange(true) }
            validated = now
        }

        override fun onLost(network: Network) {
            main.post { if (!online) onChange(false) }
        }
    }

    /** Whether the phone has any default network right now. */
    val online: Boolean get() = cm?.activeNetwork != null

    /** Registering delivers the current default network at once, so a started screen re-checks. */
    fun start() {
        if (registered || cm == null) return
        cm.registerDefaultNetworkCallback(callback)
        registered = true
    }

    fun stop() {
        if (!registered) return
        runCatching { cm?.unregisterNetworkCallback(callback) }
        registered = false
        main.removeCallbacksAndMessages(null)
    }
}

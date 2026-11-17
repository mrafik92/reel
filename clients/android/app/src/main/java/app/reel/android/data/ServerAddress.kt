package app.reel.android.data

import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/**
 * The Reel server the app talks to: scheme, host and port only (its *origin*). The API lives under
 * `/api/v1` on it. The bearer token is only ever sent to this origin.
 */
@JvmInline
value class ServerAddress private constructor(val origin: String) {
    val url: HttpUrl get() = origin.toHttpUrl()

    /** `GET` / `POST` target for an API path such as `/me`. */
    fun api(path: String): HttpUrl = (origin + API_PREFIX + path).toHttpUrl()

    /** Same scheme, host and port (the only requests that get the token). */
    fun owns(other: HttpUrl): Boolean {
        val me = url
        return other.scheme == me.scheme && other.host == me.host && other.port == me.port
    }

    override fun toString(): String = origin

    companion object {
        const val API_PREFIX = "/api/v1"

        /**
         * Parse what the user typed: `reel.example.com`, `192.168.1.5:8420`, `http://nas:8420/`,
         * or a pasted web address such as `https://reel.example.com/library/3`.
         *
         * Without a scheme, names with a dot get `https://` and LAN-style addresses (an IP, a
         * bare host name, `localhost`, or any explicit port) get `http://`, which is how Reel is
         * usually reached on a LAN or Tailscale. Returns `null` when it isn't an http(s) address.
         */
        fun parse(input: String): ServerAddress? {
            val raw = input.trim()
            if (raw.isEmpty() || raw.any { it.isWhitespace() }) return null
            val withScheme = if ("://" in raw) raw else guessScheme(raw) + "://" + raw
            val u = withScheme.toHttpUrlOrNull() ?: return null
            if (u.host.isEmpty()) return null
            val defaultPort = HttpUrl.defaultPort(u.scheme)
            val host = if (':' in u.host) "[${u.host}]" else u.host
            val port = if (u.port == defaultPort) "" else ":${u.port}"
            return ServerAddress("${u.scheme}://$host$port")
        }

        private fun guessScheme(raw: String): String {
            val authority = raw.substringBefore('/').substringBefore('?').substringAfter('@')
            val hostPart = if (authority.startsWith("[")) authority.substringBefore(']') + "]" else authority.substringBefore(':')
            val hasPort = authority.length > hostPart.length
            val host = hostPart.removePrefix("[").removeSuffix("]").lowercase()
            val isIp = ':' in host || host.all { it.isDigit() || it == '.' }
            val lanName = '.' !in host || host.endsWith(".local") || host.endsWith(".lan")
            return if (hasPort || isIp || lanName) "http" else "https"
        }

        private fun String.toHttpUrl(): HttpUrl = toHttpUrlOrNull() ?: error("invalid origin $this")
    }
}

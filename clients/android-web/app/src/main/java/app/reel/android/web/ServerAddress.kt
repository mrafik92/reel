package app.reel.android.web

import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/**
 * The Reel server the shell loads: scheme, host and port only (its *origin*). The web app is served
 * from `/` on it and the API from `/api/v1`. The WebView stays on this origin ([LinkPolicy]).
 */
@JvmInline
value class ServerAddress private constructor(val origin: String) {
    val url: HttpUrl get() = origin.toHttpUrlOrNull() ?: error("invalid origin $origin")

    /** The page the WebView opens: the web app's root. */
    val home: String get() = "$origin/"

    /** Public, unauthenticated endpoint the setup screen checks before saving. */
    val health: HttpUrl get() = "$origin/api/v1/health".toHttpUrlOrNull()!!

    /** Same scheme, host and port. */
    fun owns(other: HttpUrl): Boolean {
        val me = url
        return other.scheme == me.scheme && other.host == me.host && other.port == me.port
    }

    override fun toString(): String = origin

    companion object {
        /**
         * Parse what the user typed: `reel.example.com`, `192.168.1.5:8420`, `http://nas:8420/`,
         * a Tailscale address (`100.101.102.103:8420`, `reel.tail1234.ts.net`) or a pasted web address
         * such as `https://reel.example.com/library/3`.
         *
         * Without a scheme, names with a dot get `https://` and LAN-style addresses (an IP, a bare
         * host name, `localhost`, `.local`/`.lan`, or any explicit port) get `http://`, which is how
         * Reel is usually reached on a LAN or Tailscale. Returns `null` when it isn't an http(s) address.
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
    }
}

package app.reel.android.web

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** Where a navigation from the page goes (#225: the WebView is scoped to the Reel origin). */
enum class LinkTarget {
    /** Same origin as the configured server: load it in the WebView. */
    IN_APP,

    /** Another web site, or mail/phone: hand it to the system (browser, mail app, dialer). */
    EXTERNAL,

    /** Anything else (`intent:`, `javascript:`, `file:`, `content:`, `data:`, app schemes): ignore it. */
    BLOCKED,
}

object LinkPolicy {
    private val externalSchemes = setOf("mailto", "tel")

    fun classify(server: ServerAddress, url: String): LinkTarget {
        val scheme = url.substringBefore(':', missingDelimiterValue = "").trim().lowercase()
        return when {
            scheme == "http" || scheme == "https" -> {
                val parsed = url.toHttpUrlOrNull() ?: return LinkTarget.BLOCKED
                if (server.owns(parsed)) LinkTarget.IN_APP else LinkTarget.EXTERNAL
            }
            scheme in externalSchemes -> LinkTarget.EXTERNAL
            else -> LinkTarget.BLOCKED
        }
    }
}

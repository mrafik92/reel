package app.reel.android.web

/**
 * Why the page couldn't load (#228). Only main-frame failures get here; a failed image or API call is
 * the web app's to handle. Each kind has its own plain-language message on the error panel; the raw
 * `net::ERR_*` name is kept as a small detail line.
 */
enum class LoadProblem(
    /** Worth retrying by itself when the network returns or after a backoff delay. */
    val autoRetry: Boolean,
) {
    /** The phone has no network (airplane mode, no Wi-Fi or mobile data). */
    OFFLINE(autoRetry = true),

    /** The server's name doesn't resolve: a Tailscale or LAN name while off that network, or a typo. */
    UNKNOWN_HOST(autoRetry = true),

    /** Something answered at that address but refused the connection: wrong port, or Reel isn't running. */
    REFUSED(autoRetry = true),

    /** No answer in time: an unroutable address, off the LAN or Tailscale, or a hung server. */
    TIMED_OUT(autoRetry = true),

    /** Any other network failure (reset, closed, unreachable network). */
    NETWORK(autoRetry = true),

    /** The server answered the page with HTTP 5xx (restarting, or behind a broken proxy). */
    SERVER(autoRetry = true),

    /** Chromium never connects to this port (`ERR_UNSAFE_PORT`: 1–1023 well-known ports, 6000, 10080…). */
    BLOCKED_PORT(autoRetry = false),

    /** The certificate isn't trusted. Never retried: the shell doesn't proceed past TLS errors. */
    TLS(autoRetry = false),

    /** The WebView renderer died. The user reloads. */
    CRASH(autoRetry = false),
}

object LoadErrors {
    /**
     * How long a main-frame document request may go without the server answering (the navigation
     * committing) before the shell gives up and shows [LoadProblem.TIMED_OUT].
     *
     * Chromium's own limit is the OS TCP connect timeout: about two minutes on Android, all of it a
     * blank page. 20 s is far above a slow legitimate answer: the document is a small `index.html`,
     * and even a cold Tailscale path relayed through a distant DERP server plus a phone radio waking
     * up costs a few seconds. It only covers the wait for the server's answer; the web app's scripts,
     * images and API calls load after the commit and aren't timed.
     */
    const val LOAD_TIMEOUT_MS = 20_000L

    // WebViewClient.ERROR_* (copied so the classification is testable on the JVM).
    const val ERROR_HOST_LOOKUP = -2
    const val ERROR_CONNECT = -6
    const val ERROR_TIMEOUT = -8
    const val ERROR_FAILED_SSL_HANDSHAKE = -11

    /** The net error name in a WebView error description, such as `net::ERR_NAME_NOT_RESOLVED`. */
    fun netError(description: CharSequence?): String? =
        description?.let { Regex("net::ERR_[A-Z0-9_]+").find(it)?.value }

    /**
     * Classify a main-frame error from `WebViewClient.onReceivedError` ([code] is one of its
     * `ERROR_*` constants, [description] usually a `net::ERR_*` name). [online] is whether the phone
     * has a network at all; without one, every failure is [LoadProblem.OFFLINE].
     */
    fun classify(code: Int, description: CharSequence?, online: Boolean): LoadProblem {
        if (!online) return LoadProblem.OFFLINE
        val name = netError(description)?.removePrefix("net::ERR_")
        if (name != null) {
            when {
                name == "INTERNET_DISCONNECTED" -> return LoadProblem.OFFLINE
                name == "NAME_NOT_RESOLVED" || name == "NAME_RESOLUTION_FAILED" ||
                    name == "DNS_TIMED_OUT" -> return LoadProblem.UNKNOWN_HOST
                name == "CONNECTION_REFUSED" -> return LoadProblem.REFUSED
                name == "UNSAFE_PORT" -> return LoadProblem.BLOCKED_PORT
                name == "TIMED_OUT" || name == "CONNECTION_TIMED_OUT" ||
                    name == "ADDRESS_UNREACHABLE" -> return LoadProblem.TIMED_OUT
                name.startsWith("CERT_") || name.startsWith("SSL_") ||
                    name == "BAD_SSL_CLIENT_AUTH_CERT" -> return LoadProblem.TLS
            }
        }
        return when (code) {
            ERROR_HOST_LOOKUP -> LoadProblem.UNKNOWN_HOST
            ERROR_TIMEOUT -> LoadProblem.TIMED_OUT
            ERROR_FAILED_SSL_HANDSHAKE -> LoadProblem.TLS
            else -> LoadProblem.NETWORK // ERROR_CONNECT and the rest: reset, closed, aborted
        }
    }

    /** A main-frame HTTP status that takes over the screen, or null when the page is the web app's. */
    fun fromHttpStatus(status: Int): LoadProblem? = if (status >= 500) LoadProblem.SERVER else null
}

/**
 * When the shell retries by itself while the error panel is up (#228). A network coming back (or a new
 * default network such as Tailscale connecting) starts a fresh run; each failed attempt waits twice as
 * long, and after [delaysMs] runs out the panel waits for the next network change or a tap on Retry.
 * A working page is never reloaded because the network changed.
 */
object RetryPolicy {
    /** 1, 2, 4, 8, 16, 32 s: about a minute of trying, plus each attempt's own load time. */
    val delaysMs: List<Long> = listOf(1_000, 2_000, 4_000, 8_000, 16_000, 32_000)

    /** Delay before automatic attempt number [attempt] (0-based), or null when the run is over. */
    fun delayFor(attempt: Int): Long? = delaysMs.getOrNull(attempt)

    /** What to do about the error panel when something changes. */
    fun next(problem: LoadProblem?, online: Boolean, attempt: Int): RetryStep = when {
        problem == null -> RetryStep.Idle // the page works: nothing to do, whatever the network does
        !problem.autoRetry -> RetryStep.Idle
        !online -> RetryStep.WaitForNetwork
        else -> delayFor(attempt)?.let { RetryStep.RetryIn(it) } ?: RetryStep.Idle
    }
}

sealed interface RetryStep {
    /** No automatic retry; the user taps Retry or Change server. */
    data object Idle : RetryStep

    /** Offline: retry as soon as a network is available. */
    data object WaitForNetwork : RetryStep

    /** Retry after [delayMs]. */
    data class RetryIn(val delayMs: Long) : RetryStep
}

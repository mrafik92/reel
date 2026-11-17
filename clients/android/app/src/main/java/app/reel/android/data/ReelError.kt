package app.reel.android.data

/**
 * Why an API call failed, classified like the web client (#90) so every screen can say the same
 * thing the same way.
 */
sealed class ReelError(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /** The device has no network at all. */
    class Offline(cause: Throwable? = null) : ReelError("You're offline", cause)

    /** The server didn't answer within the time limit. */
    class Timeout(cause: Throwable? = null) : ReelError("Reel took too long to answer", cause)

    /** The connection failed: wrong address, server down, DNS, TLS, a reset connection. */
    class Unreachable(cause: Throwable? = null) : ReelError("Can't reach Reel", cause)

    /**
     * 401 on a signed-in route: the session expired or was revoked. The app clears the token
     * and goes back to sign-in. (A wrong password at sign-in is an [Http] 401 instead.)
     */
    class Unauthorized(message: String = "Your session has ended. Please sign in again.") : ReelError(message)

    /**
     * 429: locked out (login backoff) or a limit was hit. [retryAfterS] comes from `Retry-After`;
     * [body] is the raw error JSON (IPTV "busy" answers name who holds the provider).
     */
    class RateLimited(message: String, val retryAfterS: Long?, val body: String? = null) : ReelError(message)

    /** Any other non-2xx answer. [body] is the raw error JSON (some carry extra fields, e.g. `takeover`). */
    class Http(val status: Int, message: String, val body: String? = null) : ReelError(message)

    /** A 2xx answer the app couldn't parse (an older or newer server, or not Reel at all). */
    class BadResponse(cause: Throwable? = null) : ReelError("Unexpected answer from the server", cause)

    /** What a screen shows, with the wait spelled out for lockouts. */
    val userMessage: String
        get() = when (this) {
            is RateLimited -> retryAfterS?.let { "$message Try again in ${formatWait(it)}." } ?: message.orEmpty()
            else -> message ?: "Something went wrong"
        }

    companion object {
        /** Messages for statuses whose body carried no `error` (same wording as the web). */
        fun friendly(status: Int): String = when (status) {
            401 -> "Your session has ended. Please sign in again."
            403 -> "You don't have access to that"
            404 -> "Not found"
            409 -> "That already exists"
            424 -> "The server isn't fully set up for this yet"
            429 -> "Too many requests."
            502 -> "The server couldn't reach an external service"
            else -> "Server error ($status)"
        }

        fun formatWait(seconds: Long): String = when {
            seconds < 60 -> "$seconds s"
            seconds < 3600 -> "${(seconds + 59) / 60} min"
            else -> "${(seconds + 3599) / 3600} h"
        }
    }
}

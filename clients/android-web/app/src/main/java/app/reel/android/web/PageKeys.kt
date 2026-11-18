package app.reel.android.web

/**
 * The Escape key events the shell sends the page for Back (#227), keyed by their `downTime`, so the
 * shell recognises them when WebView hands them back through `WebViewClient.onUnhandledKeyEvent`
 * (the page didn't `preventDefault()` them).
 *
 * A key sent with `fallback = true` runs the shell's own Back ([BackPolicy.whenPageIgnored]) when its
 * key-down comes back unhandled. A key sent with `fallback = false` only takes a text field's focus
 * (the web app's Escape there leaves the field) and is swallowed either way.
 */
class PageKeys(private val capacity: Int = 16) {
    private class Sent(val downTime: Long, val fallback: Boolean)

    private val sent = ArrayDeque<Sent>()
    private var last = Long.MIN_VALUE

    /** A `downTime` for the next key: never repeats, even for two presses in the same millisecond. */
    fun nextDownTime(now: Long): Long {
        last = if (now > last) now else last + 1
        return last
    }

    fun sent(downTime: Long, fallback: Boolean) {
        sent.addLast(Sent(downTime, fallback))
        // A key-down the page handled never comes back; its key-up normally does. The cap keeps a lost
        // key-up from growing the list.
        while (sent.size > capacity) sent.removeFirst()
    }

    /** What to do with a key event WebView returned unhandled. */
    fun unhandled(downTime: Long, isDown: Boolean): Unhandled {
        val i = sent.indexOfFirst { it.downTime == downTime }
        if (i < 0) return Unhandled.NOT_OURS
        if (!isDown) {
            sent.removeAt(i)
            return Unhandled.SWALLOW
        }
        return if (sent[i].fallback) Unhandled.FALLBACK else Unhandled.SWALLOW
    }

    enum class Unhandled {
        /** Not a key the shell sent: let WebView's default handling have it. */
        NOT_OURS,

        /** Ours, nothing more to do. */
        SWALLOW,

        /** The page ignored Back: the shell goes back or leaves. */
        FALLBACK,
    }
}

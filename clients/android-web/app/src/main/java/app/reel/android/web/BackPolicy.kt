package app.reel.android.web

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** What one press of the system Back does. */
enum class BackAction {
    /** Leave full-screen video; the page stays where it was. */
    EXIT_FULL_SCREEN,

    /** Send Escape to the page and let the web app decide ([BackPolicy.whenPageIgnored] if it doesn't). */
    ASK_PAGE,

    /** `WebView.goBack()`. */
    GO_BACK,

    /** Move the task to the back; the page and its session stay alive. */
    LEAVE,
}

/**
 * The order of Back in the shell (#227):
 *
 * 1. Full-screen video exits full screen.
 * 2. The native error panel (no page to ask) walks the WebView history, or leaves on the first page.
 * 3. Otherwise the page gets an Escape key, the web app's Back key on every platform: it closes its own
 *    sheets, pickers and player layers, and on any route but `/` goes back in its history itself
 *    (`history.back()` from the page isn't affected by Chromium skipping entries that were created
 *    without a user gesture, unlike `WebView.canGoBack()`).
 * 4. If the page leaves Escape unhandled (it's on `/`, signed out, or isn't the Reel app), the shell goes
 *    back in the WebView history when it can and the page isn't the root route; otherwise it leaves.
 */
object BackPolicy {
    fun onBack(fullScreen: Boolean, errorShown: Boolean, hasPage: Boolean, canGoBack: Boolean): BackAction = when {
        fullScreen -> BackAction.EXIT_FULL_SCREEN
        !hasPage -> BackAction.LEAVE
        errorShown -> if (canGoBack) BackAction.GO_BACK else BackAction.LEAVE
        else -> BackAction.ASK_PAGE
    }

    /** The page didn't handle Escape. */
    fun whenPageIgnored(canGoBack: Boolean, atRoot: Boolean): BackAction =
        if (canGoBack && !atRoot) BackAction.GO_BACK else BackAction.LEAVE

    /** True when [url] is the web app's root route (`/`, any query or fragment) on [server]'s origin. */
    fun isRoot(server: ServerAddress, url: String?): Boolean {
        val u = url?.toHttpUrlOrNull() ?: return false
        if (LinkPolicy.classify(server, url) != LinkTarget.IN_APP) return false
        return u.encodedPath == "/"
    }
}

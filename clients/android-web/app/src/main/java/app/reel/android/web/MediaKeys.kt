package app.reel.android.web

import android.view.KeyEvent

/**
 * Media keys for the web player (#230). WebView turns Android's media keys into the DOM keys the web
 * app already handles (`MediaPlayPause`, `MediaPlay`, `MediaPause`, `MediaFastForward`, …), except
 * `KEYCODE_HEADSETHOOK`: the single button on wired headsets, which it maps to no DOM key, so the press
 * did nothing. The shell sends it to the page as play/pause instead, as Android's own players do.
 */
object MediaKeys {
    /** The key code to send the page in place of [keyCode], or null to let WebView handle it as is. */
    fun forPage(keyCode: Int): Int? = when (keyCode) {
        KeyEvent.KEYCODE_HEADSETHOOK -> KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE
        else -> null
    }
}

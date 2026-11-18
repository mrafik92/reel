package app.reel.android.web

import android.annotation.SuppressLint
import android.view.View
import android.webkit.WebView

/**
 * WebView defaults that make Reel Web feel like a web page rather than an app (#250), turned off:
 * Android 12+'s stretch overscroll (it stretched the whole WebView, the fixed bottom bar included),
 * the native scrollbar (drawn over the bottom bar), pinch and double-tap zoom, and long-press text
 * selection outside text fields. The web app's own scroll containers and inputs are unaffected.
 */
object AppFeel {
    @SuppressLint("ClickableViewAccessibility")
    fun apply(view: WebView) {
        view.overScrollMode = View.OVER_SCROLL_NEVER
        view.isVerticalScrollBarEnabled = false
        view.isHorizontalScrollBarEnabled = false
        with(view.settings) {
            setSupportZoom(false)
            builtInZoomControls = false
            displayZoomControls = false
        }
        // Long-press starts a text selection (with a haptic buzz) anywhere on the page; native apps only
        // allow that in text fields. Returning true consumes the long-press; the haptic is switched off
        // so a consumed press doesn't vibrate.
        view.isHapticFeedbackEnabled = false
        view.setOnLongClickListener { v -> !allowsLongPress((v as WebView).hitTestResult.type) }
    }

    /** Long-press (selection, paste) stays available only in editable fields. */
    fun allowsLongPress(hitType: Int): Boolean = hitType == WebView.HitTestResult.EDIT_TEXT_TYPE
}

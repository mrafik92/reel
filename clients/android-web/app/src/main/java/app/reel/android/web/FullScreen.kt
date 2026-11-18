package app.reel.android.web

import android.app.Activity
import android.content.pm.ActivityInfo
import android.view.View
import android.view.ViewGroup
import android.webkit.WebChromeClient
import android.widget.FrameLayout
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * Full-screen video (#227). The web player's full-screen button (the Fullscreen API) and a `<video>`'s
 * own control both end in `WebChromeClient.onShowCustomView`: the view WebView hands over goes into
 * [layer], which covers the page edge to edge. While it's up the system bars are hidden (a swipe shows
 * them briefly) and the screen turns to landscape; leaving restores the bars and the orientation the
 * activity had, and the page is where it was.
 *
 * Leaving happens three ways: the page exits full screen (the player's button, or the player closing at
 * the end of a title) → `onHideCustomView` → [hide]; Back → [exit], which also tells WebView; and the
 * renderer going away → [hide].
 */
class FullScreen(private val activity: Activity, private val layer: FrameLayout, private val tv: Boolean) {
    private var view: View? = null
    private var callback: WebChromeClient.CustomViewCallback? = null
    private var savedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED

    val isShowing: Boolean get() = view != null

    private val bars: WindowInsetsControllerCompat
        get() = WindowCompat.getInsetsController(activity.window, activity.window.decorView)

    fun show(custom: View, onHidden: WebChromeClient.CustomViewCallback) {
        if (view != null) {
            // Already full screen (a second element asked): keep the first, refuse this one.
            onHidden.onCustomViewHidden()
            return
        }
        view = custom
        callback = onHidden
        (custom.parent as? ViewGroup)?.removeView(custom)
        layer.addView(custom, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        layer.visibility = View.VISIBLE
        savedOrientation = activity.requestedOrientation
        activity.requestedOrientation = orientationFor(tv, savedOrientation)
        hideBars()
    }

    /** Re-hide the bars after the activity comes back (another app or a dialog may have shown them). */
    fun reapply() {
        if (isShowing) hideBars()
    }

    /** Back while full screen: tear down here and tell WebView, which exits the page's full screen. */
    fun exit() {
        val cb = callback
        hide()
        cb?.onCustomViewHidden()
    }

    /** Tear down (idempotent: WebView may call `onHideCustomView` after [exit] already did this). */
    fun hide() {
        val v = view ?: return
        view = null
        callback = null
        layer.removeView(v)
        layer.visibility = View.GONE
        bars.show(WindowInsetsCompat.Type.systemBars())
        activity.requestedOrientation = savedOrientation
    }

    private fun hideBars() {
        bars.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        bars.hide(WindowInsetsCompat.Type.systemBars())
    }

    companion object {
        /**
         * Films and series are landscape: a phone turns to landscape for full screen even with
         * auto-rotate off, and either landscape side works. A TV is already landscape, so its setting
         * is kept.
         */
        fun orientationFor(tv: Boolean, current: Int): Int =
            if (tv) current else ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
    }
}

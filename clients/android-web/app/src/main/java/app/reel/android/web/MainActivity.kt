package app.reel.android.web

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.net.http.SslError
import android.os.Build
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.RenderProcessGoneDetail
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.FrameLayout
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat

/**
 * Reel Web: the server's own web app in a WebView (#225). The shell adds what a browser tab can't:
 * a remembered server, Android Back, and a native screen when the server can't be loaded.
 *
 * Security (#226): navigation stays on the configured origin ([LinkPolicy]), TLS errors always cancel,
 * file and content access are off, mixed content is blocked, and there is no JavaScript bridge.
 */
class MainActivity : ComponentActivity() {
    private val store by lazy { ServerStore(this) }
    private var configured: ServerAddress? = null
    private val server: ServerAddress get() = checkNotNull(configured)
    private var web: WebView? = null

    private lateinit var container: FrameLayout
    private lateinit var progress: ProgressBar
    private lateinit var errorPanel: View
    private lateinit var errorTitle: TextView
    private lateinit var errorMessage: TextView

    /** Set by an error callback for the page being loaded; `onPageFinished` then keeps the panel up. */
    private var failed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        configured = store.server ?: run {
            startActivity(Intent(this, SetupActivity::class.java))
            finish()
            return
        }
        setContentView(R.layout.activity_main)
        findViewById<View>(R.id.main_root).padForSystemBars()
        container = findViewById(R.id.web_container)
        progress = findViewById(R.id.progress)
        errorPanel = findViewById(R.id.error_panel)
        errorTitle = findViewById(R.id.error_title)
        errorMessage = findViewById(R.id.error_message)
        findViewById<Button>(R.id.retry).setOnClickListener { retry() }
        findViewById<Button>(R.id.change_server).setOnClickListener { changeServer() }

        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        val view = createWebView()
        if (savedInstanceState?.let { view.restoreState(it) } == null) view.loadUrl(server.home)

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                val w = web
                if (w != null && w.canGoBack()) {
                    hideError()
                    w.goBack()
                } else {
                    isEnabled = false
                    onBackPressedDispatcher.onBackPressed()
                    isEnabled = true
                }
            }
        })
        publishShortcut()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        web?.saveState(outState)
    }

    override fun onResume() {
        super.onResume()
        web?.onResume()
    }

    override fun onPause() {
        web?.onPause()
        super.onPause()
    }

    override fun onStop() {
        // The web session is in localStorage; cookies are flushed too in case the server sets any.
        CookieManager.getInstance().flush()
        super.onStop()
    }

    override fun onDestroy() {
        destroyWebView()
        super.onDestroy()
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun createWebView(): WebView {
        val view = WebView(this)
        view.layoutParams = FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        view.setBackgroundColor(getColor(R.color.bg))
        with(view.settings) {
            javaScriptEnabled = true
            domStorageEnabled = true // the web app keeps its session token in localStorage
            mediaPlaybackRequiresUserGesture = false // autoplay of the next episode and Live TV zaps
            allowFileAccess = false
            allowContentAccess = false
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            setSupportMultipleWindows(false) // target=_blank / window.open navigate here, through LinkPolicy
            javaScriptCanOpenWindowsAutomatically = false
            setGeolocationEnabled(false)
            userAgentString = UserAgent.extend(userAgentString, BuildConfig.VERSION_NAME, isTelevision())
        }
        CookieManager.getInstance().setAcceptCookie(true)
        CookieManager.getInstance().setAcceptThirdPartyCookies(view, false)
        view.webViewClient = ShellClient()
        view.webChromeClient = object : WebChromeClient() {
            override fun onProgressChanged(view: WebView, newProgress: Int) {
                progress.progress = newProgress
                progress.visibility = if (newProgress in 1..99) View.VISIBLE else View.GONE
            }
        }
        container.addView(view)
        web = view
        return view
    }

    private fun destroyWebView() {
        web?.let {
            container.removeView(it)
            it.destroy()
        }
        web = null
    }

    private inner class ShellClient : WebViewClient() {
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            val url = request.url.toString()
            val target = LinkPolicy.classify(server, url)
            if (!request.isForMainFrame) {
                // Embedded frames (a trailer player) render inside the page; only web schemes may load.
                return target == LinkTarget.BLOCKED
            }
            return when (target) {
                LinkTarget.IN_APP -> false
                LinkTarget.EXTERNAL -> {
                    openExternally(request.url)
                    true
                }
                LinkTarget.BLOCKED -> true
            }
        }

        override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
            failed = false
        }

        override fun onPageFinished(view: WebView, url: String) {
            if (!failed) hideError()
        }

        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
            if (!request.isForMainFrame) return // a failed image or API call is the web app's to handle
            showError(getString(R.string.error_unreachable_title), getString(R.string.error_unreachable, server.origin, error.description))
        }

        override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
            if (request.isForMainFrame && response.statusCode >= 500) {
                showError(getString(R.string.error_server_title), getString(R.string.error_server, server.origin, response.statusCode))
            }
        }

        @SuppressLint("WebViewClientOnReceivedSslError")
        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            // Never proceed past a certificate error (#225), not even for the configured server.
            handler.cancel()
            if (error.url == null || LinkPolicy.classify(server, error.url) == LinkTarget.IN_APP) {
                showError(getString(R.string.error_tls_title), getString(R.string.error_tls, server.origin))
            }
        }

        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
            // The renderer crashed or was killed for memory: drop this WebView and offer a reload,
            // instead of letting the whole app crash.
            destroyWebView()
            createWebView()
            showError(getString(R.string.error_crash_title), getString(R.string.error_crash))
            return true
        }
    }

    private fun openExternally(uri: Uri) {
        val intent = Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, R.string.no_app_for_link, Toast.LENGTH_SHORT).show()
        }
    }

    private fun showError(title: String, message: String) {
        failed = true
        errorTitle.text = title
        errorMessage.text = message
        errorPanel.visibility = View.VISIBLE
        web?.visibility = View.INVISIBLE
        findViewById<View>(R.id.retry).requestFocus()
    }

    private fun hideError() {
        errorPanel.visibility = View.GONE
        web?.visibility = View.VISIBLE
    }

    private fun retry() {
        val w = web ?: createWebView()
        hideError()
        // After a crash, or when nothing loaded yet, start from the web app's root.
        if (w.url.isNullOrEmpty() || w.url == "about:blank") w.loadUrl(server.home) else w.reload()
    }

    private fun changeServer() {
        startActivity(Intent(this, SetupActivity::class.java).setAction(SetupActivity.ACTION_CHANGE_SERVER))
    }

    /** Long-press the launcher icon → "Change server": the page can't reach native UI (no bridge). */
    private fun publishShortcut() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N_MR1) return
        val shortcut = ShortcutInfoCompat.Builder(this, "change-server")
            .setShortLabel(getString(R.string.shortcut_change_server))
            .setIcon(IconCompat.createWithResource(this, R.drawable.ic_shortcut_server))
            .setIntent(Intent(this, SetupActivity::class.java).setAction(SetupActivity.ACTION_CHANGE_SERVER))
            .build()
        ShortcutManagerCompat.pushDynamicShortcut(this, shortcut)
    }
}

package app.reel.android.web

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.net.http.SslError
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.os.SystemClock
import android.util.Log
import android.view.KeyEvent
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
 * a remembered server, Android Back (see [BackPolicy] for the order), full-screen video ([FullScreen]),
 * and a native screen when the server can't be loaded, which retries by itself when the network returns
 * (#228: [LoadErrors], [RetryPolicy], [NetworkWatch]).
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
    private lateinit var fullScreen: FullScreen
    private val pageKeys = PageKeys()
    private lateinit var progress: ProgressBar
    private lateinit var errorPanel: View
    private lateinit var errorTitle: TextView
    private lateinit var errorMessage: TextView
    private lateinit var errorDetail: TextView
    private lateinit var errorStatus: TextView
    private lateinit var network: NetworkWatch
    private val handler = Handler(Looper.getMainLooper())

    /** Set by an error callback for the page being loaded; `onPageFinished` then keeps the panel up. */
    private var failed = false

    /** What the error panel shows, or null while it's hidden. */
    private var problem: LoadProblem? = null

    /** The main-frame URL that failed, retried by the panel. */
    private var failingUrl: String? = null

    /** The main-frame URL being loaded, while the load timeout ([LoadErrors.LOAD_TIMEOUT_MS]) runs. */
    private var pendingUrl: String? = null

    /** Automatic retries so far in this run ([RetryPolicy]). */
    private var attempt = 0

    /** A retry is loading behind the panel; a network event then doesn't start another one. */
    private var retrying = false
    private var retryAt = 0L
    private var firstPageLogged = false

    private val loadTimeout = Runnable { onLoadTimeout() }
    private val autoRetry = Runnable { retry(auto = true) }
    private val countdown = object : Runnable {
        override fun run() {
            val left = retryAt - SystemClock.uptimeMillis()
            if (left <= 0) return
            setStatus(getString(R.string.retry_in, ((left + 999) / 1000).toInt()))
            handler.postDelayed(this, (left % 1000).takeIf { it > 0 } ?: 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        configured = store.server ?: run {
            startActivity(Intent(this, SetupActivity::class.java))
            finish()
            return
        }
        setContentView(R.layout.activity_main)
        findViewById<View>(R.id.content).padForSystemBars()
        fullScreen = FullScreen(this, findViewById(R.id.full_screen), isTelevision())
        container = findViewById(R.id.web_container)
        progress = findViewById(R.id.progress)
        errorPanel = findViewById(R.id.error_panel)
        errorTitle = findViewById(R.id.error_title)
        errorMessage = findViewById(R.id.error_message)
        errorDetail = findViewById(R.id.error_detail)
        errorStatus = findViewById(R.id.error_status)
        findViewById<Button>(R.id.retry).setOnClickListener { retry(auto = false) }
        findViewById<Button>(R.id.change_server).setOnClickListener { changeServer() }
        network = NetworkWatch(this, ::onNetworkChanged)

        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        val view = createWebView()
        val restored = savedInstanceState?.let { view.restoreState(it) } != null
        if (restored && view.url?.let { LinkPolicy.classify(server, it) } == LinkTarget.IN_APP) {
            watchLoad(view.url)
        } else {
            load(view, server.home)
        }

        // Always enabled: whether Back stays in the app is the page's call, made after the press (so the
        // predictive back-to-home preview can't be shown; Back still works the same with it on).
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = onBack()
        })
        publishShortcut()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        web?.saveState(outState)
    }

    override fun onStart() {
        super.onStart()
        if (!::network.isInitialized) return
        // A server change replaces this task (SetupActivity), but never show the old origin's page if it didn't.
        val current = store.server
        if (current != null && current != configured) switchServer(current)
        network.start() // delivers the current network at once: a panel left up retries on return
    }

    override fun onResume() {
        super.onResume()
        web?.onResume()
        if (::fullScreen.isInitialized) fullScreen.reapply()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && ::fullScreen.isInitialized) fullScreen.reapply()
    }

    /**
     * The wired-headset button reaches the page as play/pause ([MediaKeys]). It goes the normal way to the
     * focused view: the WebView, or WebView's own view while full screen. Other keys are untouched.
     * (`Activity.dispatchKeyEvent` is public; androidx core marks its own override `@RestrictTo`, which lint
     * then reports for every subclass.)
     */
    @SuppressLint("RestrictedApi")
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val code = MediaKeys.forPage(event.keyCode) ?: return super.dispatchKeyEvent(event)
        return super.dispatchKeyEvent(KeyEvent(event.downTime, event.eventTime, event.action, code, event.repeatCount))
    }

    override fun onPause() {
        web?.onPause()
        super.onPause()
    }

    override fun onStop() {
        // The web session is in localStorage; cookies are flushed too in case the server sets any.
        CookieManager.getInstance().flush()
        if (::network.isInitialized) network.stop()
        stopAutoRetry()
        super.onStop()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        if (::fullScreen.isInitialized) fullScreen.hide()
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

            override fun onShowCustomView(view: View, callback: CustomViewCallback) = fullScreen.show(view, callback)

            override fun onHideCustomView() = fullScreen.hide()
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
                LinkTarget.IN_APP -> {
                    watchLoad(url)
                    false
                }
                LinkTarget.EXTERNAL -> {
                    openExternally(request.url)
                    true
                }
                LinkTarget.BLOCKED -> true
            }
        }

        override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
            debug("started $url")
            loadAnswered()
            failed = false
        }

        override fun doUpdateVisitedHistory(view: WebView, url: String, isReload: Boolean) {
            loadAnswered() // also covers same-document history steps, which have no onPageStarted
        }

        override fun onPageFinished(view: WebView, url: String) {
            // Not loadAnswered(): a stopped load can finish after the next one started, and must not
            // cancel that one's timeout. A real answer commits first (onPageStarted).
            debug("finished $url failed=$failed")
            if (failed) return
            hideError()
            if (!firstPageLogged) {
                firstPageLogged = true
                // Cold start for the design note (#228 §6): process start → first page finished.
                debug("first page finished ${SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime()} ms after process start")
            }
        }

        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
            if (!request.isForMainFrame) return // a failed image or API call is the web app's to handle
            val name = LoadErrors.netError(error.description)
            debug("main-frame error ${error.errorCode} ${error.description}")
            if (name == "net::ERR_ABORTED") return // replaced by another load, or stopped by the timeout
            if (failed && problem == LoadProblem.TLS) return // keep the certificate message for this load
            failingUrl = request.url.toString()
            val kind = LoadErrors.classify(error.errorCode, error.description, network.online)
            showProblem(kind, detail = name ?: error.description?.toString())
        }

        override fun onReceivedHttpError(view: WebView, request: WebResourceRequest, response: WebResourceResponse) {
            if (!request.isForMainFrame) return
            val kind = LoadErrors.fromHttpStatus(response.statusCode) ?: return
            failingUrl = request.url.toString()
            showProblem(kind, detail = getString(R.string.error_detail_http, response.statusCode))
        }

        @SuppressLint("WebViewClientOnReceivedSslError")
        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
            // Never proceed past a certificate error (#225), not even for the configured server.
            handler.cancel()
            if (error.url == null || LinkPolicy.classify(server, error.url) == LinkTarget.IN_APP) {
                showProblem(LoadProblem.TLS, detail = null)
            }
        }

        override fun onUnhandledKeyEvent(view: WebView, event: KeyEvent) {
            if (event.keyCode == KeyEvent.KEYCODE_ESCAPE) {
                when (pageKeys.unhandled(event.downTime, event.action == KeyEvent.ACTION_DOWN)) {
                    PageKeys.Unhandled.NOT_OURS -> Unit
                    PageKeys.Unhandled.SWALLOW -> return
                    PageKeys.Unhandled.FALLBACK -> {
                        perform(BackPolicy.whenPageIgnored(view.canGoBack(), BackPolicy.isRoot(server, view.url)))
                        return
                    }
                }
            }
            super.onUnhandledKeyEvent(view, event)
        }

        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
            // The renderer crashed or was killed for memory: drop this WebView and offer a reload,
            // instead of letting the whole app crash.
            fullScreen.hide()
            destroyWebView()
            createWebView()
            failingUrl = null
            showProblem(LoadProblem.CRASH, detail = null)
            return true
        }
    }

    private fun onBack() {
        val w = web
        perform(
            BackPolicy.onBack(
                fullScreen = fullScreen.isShowing,
                errorShown = errorPanel.visibility == View.VISIBLE,
                hasPage = w != null,
                canGoBack = w?.canGoBack() == true,
            ),
        )
    }

    private fun perform(action: BackAction) {
        when (action) {
            BackAction.EXIT_FULL_SCREEN -> fullScreen.exit()
            BackAction.ASK_PAGE -> web?.let(::sendBackToPage)
            BackAction.GO_BACK -> {
                hideError()
                watchLoad(null)
                web?.goBack()
            }
            BackAction.LEAVE -> moveTaskToBack(true)
        }
    }

    /**
     * Back as the web app's Back key (Escape). When a text field has focus the keyboard is already
     * closed (the IME takes the press that closes it), and the web app's Escape there only leaves the
     * field: one Escape does that first, so the press still goes back, as it does in Chrome.
     */
    private fun sendBackToPage(w: WebView) {
        if (!w.hasFocus()) w.requestFocus()
        if (w.onCheckIsTextEditor()) sendEscape(w, fallback = false)
        sendEscape(w, fallback = true)
    }

    private fun sendEscape(w: WebView, fallback: Boolean) {
        val down = pageKeys.nextDownTime(SystemClock.uptimeMillis())
        pageKeys.sent(down, fallback)
        w.dispatchKeyEvent(KeyEvent(down, down, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ESCAPE, 0))
        w.dispatchKeyEvent(KeyEvent(down, down, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_ESCAPE, 0))
    }

    private fun openExternally(uri: Uri) {
        val intent = Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, R.string.no_app_for_link, Toast.LENGTH_SHORT).show()
        }
    }

    /** Load [url] in the main frame, with the shell's own timeout for the server's answer. */
    private fun load(w: WebView, url: String) {
        watchLoad(url)
        w.loadUrl(url)
    }

    /**
     * Start the load timeout: Chromium waits for the OS connect timeout (minutes) on an unroutable
     * address, with a blank page. Any commit, error or finish cancels it ([loadAnswered]).
     */
    private fun watchLoad(url: String?) {
        debug("load ${url ?: "(history)"}")
        pendingUrl = url
        handler.removeCallbacks(loadTimeout)
        handler.postDelayed(loadTimeout, LoadErrors.LOAD_TIMEOUT_MS)
    }

    private fun loadAnswered() {
        handler.removeCallbacks(loadTimeout)
    }

    private fun onLoadTimeout() {
        val w = web ?: return
        debug("no answer in ${LoadErrors.LOAD_TIMEOUT_MS} ms: ${pendingUrl ?: w.url}")
        failingUrl = pendingUrl ?: w.url
        w.stopLoading()
        val kind = if (network.online) LoadProblem.TIMED_OUT else LoadProblem.OFFLINE
        showProblem(kind, detail = getString(R.string.error_detail_timeout, (LoadErrors.LOAD_TIMEOUT_MS / 1000).toInt()))
    }

    private fun showProblem(kind: LoadProblem, detail: String?) {
        loadAnswered()
        val wasShown = problem != null
        failed = true
        retrying = false
        problem = kind
        val host = server.url.host
        val (title, message) = when (kind) {
            LoadProblem.OFFLINE -> R.string.error_offline_title to getString(R.string.error_offline, server.origin)
            LoadProblem.UNKNOWN_HOST -> R.string.error_unknown_host_title to getString(R.string.error_unknown_host, host)
            LoadProblem.REFUSED -> R.string.error_refused_title to getString(R.string.error_refused, server.origin)
            LoadProblem.TIMED_OUT -> R.string.error_unreachable_title to getString(R.string.error_timed_out, server.origin)
            LoadProblem.NETWORK -> R.string.error_unreachable_title to getString(R.string.error_network, server.origin)
            LoadProblem.BLOCKED_PORT -> R.string.error_blocked_port_title to getString(R.string.error_blocked_port, server.url.port)
            LoadProblem.SERVER -> R.string.error_server_title to getString(R.string.error_server, server.origin)
            LoadProblem.TLS -> R.string.error_tls_title to getString(R.string.error_tls, server.origin)
            LoadProblem.CRASH -> R.string.error_crash_title to getString(R.string.error_crash)
        }
        errorTitle.setText(title)
        errorMessage.text = message
        errorDetail.text = detail
        errorDetail.visibility = if (detail.isNullOrEmpty()) View.GONE else View.VISIBLE
        errorPanel.visibility = View.VISIBLE
        web?.visibility = View.INVISIBLE
        // Only on the way in: a failed automatic retry must not move focus off Change server (TV).
        if (!wasShown) findViewById<View>(R.id.retry).requestFocus()
        planRetry()
    }

    private fun hideError() {
        problem = null
        retrying = false
        stopAutoRetry()
        attempt = 0
        errorPanel.visibility = View.GONE
        web?.visibility = View.VISIBLE
    }

    /** Schedule the next automatic retry for the panel, or wait ([RetryPolicy]). */
    private fun planRetry() {
        stopAutoRetry()
        when (val step = RetryPolicy.next(problem, network.online, attempt)) {
            RetryStep.Idle -> setStatus(null)
            RetryStep.WaitForNetwork -> setStatus(getString(R.string.retry_waiting_network))
            is RetryStep.RetryIn -> {
                attempt++
                retryAt = SystemClock.uptimeMillis() + step.delayMs
                handler.postDelayed(autoRetry, step.delayMs)
                countdown.run()
            }
        }
    }

    private fun stopAutoRetry() {
        handler.removeCallbacks(autoRetry)
        handler.removeCallbacks(countdown)
    }

    /** From [NetworkWatch]: a new or newly validated network restarts the retry run; a working page is left alone. */
    private fun onNetworkChanged(newNetwork: Boolean) {
        if (problem == null) return
        debug("network ${if (newNetwork) "available" else "lost"} with the panel up")
        // Wi-Fi joining reports "available" and then "validated": one retry is enough. If it fails, the
        // backoff continues from there.
        if (newNetwork && retrying) return
        if (newNetwork) attempt = 0
        planRetry()
    }

    private fun setStatus(text: String?) {
        errorStatus.text = text
        errorStatus.visibility = if (text == null) View.GONE else View.VISIBLE
    }

    /**
     * Retry the failed page (or the web app's root after a crash, or when nothing loaded yet). The panel
     * stays up until the page finishes, so Chromium's error page never shows. A tap starts a new run.
     */
    private fun retry(auto: Boolean) {
        stopAutoRetry()
        if (!auto) attempt = 0
        val w = web ?: createWebView()
        val target = listOf(failingUrl, w.url).firstOrNull { it != null && LinkPolicy.classify(server, it) == LinkTarget.IN_APP }
        setStatus(getString(R.string.retry_connecting))
        retrying = true
        debug("retry (${if (auto) "automatic, attempt $attempt" else "tap"}) ${target ?: server.home}")
        load(w, target ?: server.home)
    }

    /** The configured server changed under a live activity: drop the old origin's page and history. */
    private fun switchServer(address: ServerAddress) {
        configured = address
        failingUrl = null
        hideError()
        fullScreen.hide()
        destroyWebView()
        load(createWebView(), server.home)
    }

    private fun debug(message: String) {
        if (BuildConfig.DEBUG) Log.d(TAG, message)
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

    private companion object {
        const val TAG = "ReelWeb"
    }
}

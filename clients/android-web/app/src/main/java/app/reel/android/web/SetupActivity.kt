package app.reel.android.web

import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import java.util.concurrent.Executors

/**
 * Enter or change the Reel server address. Reel stays private (LAN + Tailscale), so the check
 * can fail simply because the phone is off that network: after a failed check, "Save anyway" keeps
 * the address and the WebView's own error screen takes over.
 */
class SetupActivity : ComponentActivity() {
    private val store by lazy { ServerStore(this) }
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var pending: ServerAddress? = null

    /** A check is running. Connect stays enabled (and focused) meanwhile; disabling it would move a D-pad's focus. */
    private var checking = false

    private lateinit var input: EditText
    private lateinit var connect: Button
    private lateinit var saveAnyway: Button
    private lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_setup)
        findViewById<View>(R.id.setup_root).padForSystemBars()

        input = findViewById(R.id.server_input)
        connect = findViewById(R.id.connect)
        saveAnyway = findViewById(R.id.save_anyway)
        status = findViewById(R.id.status)

        if (savedInstanceState == null) store.server?.let { input.setText(it.origin) }
        connect.setOnClickListener { submit() }
        saveAnyway.setOnClickListener { pending?.let(::save) }
        input.setOnEditorActionListener { _, action, event ->
            val enter = event?.keyCode == KeyEvent.KEYCODE_ENTER && event.action == KeyEvent.ACTION_DOWN
            if (action == EditorInfo.IME_ACTION_GO || enter) {
                submit()
                true
            } else {
                false
            }
        }
        input.requestFocus()
    }

    override fun onDestroy() {
        io.shutdownNow()
        super.onDestroy()
    }

    private fun submit() {
        if (checking) return
        val address = ServerAddress.parse(input.text.toString())
        saveAnyway.visibility = View.GONE
        if (address == null) {
            status.setText(R.string.setup_invalid)
            return
        }
        pending = address
        checking = true
        status.text = getString(R.string.setup_checking, address.origin)
        io.execute {
            val result = ServerCheck().check(address)
            main.post { if (!isDestroyed) onChecked(address, result) }
        }
    }

    private fun onChecked(address: ServerAddress, result: CheckResult) {
        checking = false
        if (result == CheckResult.Ok) {
            save(address)
            return
        }
        status.text = when (result) {
            CheckResult.NotReel -> getString(R.string.setup_not_reel, address.origin)
            CheckResult.UnknownHost -> getString(R.string.setup_unknown_host, address.url.host)
            CheckResult.Tls -> getString(R.string.setup_tls, address.origin)
            is CheckResult.Unreachable -> getString(R.string.setup_unreachable, address.origin)
            CheckResult.Ok -> ""
        }
        // A certificate problem isn't something to save past: the WebView would refuse it anyway.
        saveAnyway.visibility = if (result == CheckResult.Tls) View.GONE else View.VISIBLE
    }

    private fun save(address: ServerAddress) {
        store.server = address
        startActivity(
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )
        finish()
    }

    companion object {
        const val ACTION_CHANGE_SERVER = "app.reel.android.web.CHANGE_SERVER"
    }
}

package app.reel.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import app.reel.android.ui.ReelRoot
import app.reel.android.ui.theme.ReelTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val kind = DeviceKind.of(this)
        setContent {
            ReelTheme(kind) {
                ReelRoot(kind)
            }
        }
    }
}

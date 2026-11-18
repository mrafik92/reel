package app.reel.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.SystemBarStyle
import androidx.compose.runtime.CompositionLocalProvider
import app.reel.android.ui.ReelRoot
import app.reel.android.ui.theme.ReelTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
        val kind = DeviceKind.of(this)
        val graph = (application as ReelApplication).graph
        setContent {
            CompositionLocalProvider(LocalAppGraph provides graph, LocalDeviceKind provides kind) {
                ReelTheme(kind) {
                    ReelRoot()
                }
            }
        }
    }
}

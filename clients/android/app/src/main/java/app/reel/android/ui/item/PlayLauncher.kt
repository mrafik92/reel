package app.reel.android.ui.item

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.navigation.NavHostController
import app.reel.android.LocalAppGraph
import app.reel.android.data.ReelApi
import app.reel.android.data.item
import app.reel.android.ui.common.Load
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Starts playback of an item known only by its id (a Home card, a show's next episode): loads its
 * detail for the file and the resume point, then opens [Routes.player].
 */
@Stable
class PlayLauncher internal constructor(
    private val api: ReelApi?,
    private val nav: NavHostController,
    private val scope: CoroutineScope,
) {
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)

    /** Play [itemId] from its resume point, or from the start when [fromStart]. */
    fun play(itemId: Long, fromStart: Boolean = false) {
        if (busy || api == null) return
        busy = true
        error = null
        scope.launch {
            try {
                val d = api.item(itemId)
                val file = d.files.firstOrNull()
                if (file == null) {
                    error = "No playable file for this title yet."
                } else {
                    val start = if (fromStart) 0.0 else ItemLogic.resumeAt(d.progress, file.durationS, d.played)
                    nav.navigate(Routes.player(file.id, d.id, start))
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = Load.Failed(e).message
            } finally {
                busy = false
            }
        }
    }
}

@Composable
fun rememberPlayLauncher(): PlayLauncher {
    val api = LocalAppGraph.current.session.api
    val nav = LocalNavController.current
    val scope = rememberCoroutineScope()
    return remember(api, nav, scope) { PlayLauncher(api, nav, scope) }
}

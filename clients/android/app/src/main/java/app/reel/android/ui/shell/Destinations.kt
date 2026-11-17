package app.reel.android.ui.shell

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.LiveTv
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.VideoLibrary
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.navigation.NavHostController
import app.reel.android.data.Me

/**
 * The app's top-level destinations, in rail / bottom-bar order. [Live] only shows for users with
 * Live TV access. Each one's screen is registered in [reelGraph].
 */
enum class TopLevel(val route: String, val label: String, val icon: ImageVector) {
    Home("home", "Home", Icons.Outlined.Home),
    Search("search", "Search", Icons.Outlined.Search),
    Live("live", "Live TV", Icons.Outlined.LiveTv),
    Library("library", "Library", Icons.Outlined.VideoLibrary),
    Settings("settings", "Settings", Icons.Outlined.Settings),
    ;

    fun visibleTo(user: Me): Boolean = this != Live || user.canWatchLive

    companion object {
        fun visibleTo(user: Me): List<TopLevel> = entries.filter { it.visibleTo(user) }
    }
}

/** Routes below the top level. Add a builder here and its `composable(...)` in [reelGraph]. */
object Routes {
    const val ITEM = "item/{id}"

    /** The VOD player (#190): full screen, no rail or bottom bar. */
    const val PLAYER = "player/{fileId}?item={itemId}&start={startS}"

    fun item(id: Long) = "item/$id"

    /** Play [fileId] of item [itemId] from [startS] seconds (absolute; see `resumePositionS`). */
    fun player(fileId: Long, itemId: Long, startS: Double) = "player/$fileId?item=$itemId&start=$startS"

    /** Routes that take the whole screen. */
    fun isFullScreen(route: String?) = route == PLAYER || route == app.reel.android.ui.vod.VodRoutes.PLAYER
}

/** The shell's nav controller; screens navigate with `LocalNavController.current.navigate(Routes.item(id))`. */
val LocalNavController = staticCompositionLocalOf<NavHostController> { error("no NavController") }

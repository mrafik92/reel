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

    fun item(id: Long) = "item/$id"

    /** One library's grid (Home's *See all*); the Library tab itself opens the first library. */
    const val LIBRARY = "library/{id}"

    fun library(id: Long) = "library/$id"

    /**
     * The VOD player (#190): play [fileId] for [itemId] (progress is credited to it) from
     * [startS] seconds. The contract between the browse screens (#188) and the player; see
     * [PlayerArgs].
     */
    const val PLAYER = "player/{fileId}/{itemId}/{startS}"

    fun player(fileId: Long, itemId: Long, startS: Double) = "player/$fileId/$itemId/$startS"

    /** Routes that take the whole screen (no rail or bottom bar). */
    fun isFullScreen(route: String?) = route == PLAYER
}

/** The [Routes.PLAYER] arguments: `Player(fileId: Long, itemId: Long, startS: Double)`. */
data class PlayerArgs(val fileId: Long, val itemId: Long, val startS: Double) {
    companion object {
        /** From a back-stack entry's arguments (`startS` travels as a string: NavType has no Double). */
        fun from(args: android.os.Bundle?): PlayerArgs = PlayerArgs(
            fileId = args?.getLong("fileId") ?: 0,
            itemId = args?.getLong("itemId") ?: 0,
            startS = args?.getString("startS")?.toDoubleOrNull() ?: 0.0,
        )
    }
}

/** The shell's nav controller; screens navigate with `LocalNavController.current.navigate(Routes.item(id))`. */
val LocalNavController = staticCompositionLocalOf<NavHostController> { error("no NavController") }

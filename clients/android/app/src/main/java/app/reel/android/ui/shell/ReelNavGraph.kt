package app.reel.android.ui.shell

import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavType
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import app.reel.android.data.Me
import app.reel.android.player.PlayerScreen
import app.reel.android.ui.vod.vodGraph
import app.reel.android.ui.home.HomeScreen
import app.reel.android.ui.item.ItemScreen
import app.reel.android.ui.library.LibraryScreen
import app.reel.android.ui.search.SearchScreen
import app.reel.android.ui.settings.SettingsScreen
import app.reel.android.ui.stubs.LivePlaceholder

/**
 * The destination registry: every screen of the app is registered here. Screen tickets replace a
 * placeholder with their screen (#188 Home/Library/item, #189 Search, #191 Live TV, #194 Settings)
 * and add their own routes (player, season, …) next to these.
 */
fun NavGraphBuilder.reelGraph(user: Me) {
    composable(TopLevel.Home.route) { HomeScreen() }
    composable(TopLevel.Search.route) { SearchScreen() }
    if (user.canWatchLive) composable(TopLevel.Live.route) { LivePlaceholder() }
    composable(TopLevel.Library.route) { LibraryScreen() }
    composable(TopLevel.Settings.route) { SettingsScreen(user) }

    composable(Routes.ITEM, arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
        ItemScreen(entry.arguments?.getLong("id") ?: 0)
    }
    composable(Routes.LIBRARY, arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
        LibraryScreen(initialLibrary = entry.arguments?.getLong("id"))
    }
    composable(
        Routes.PLAYER,
        arguments = listOf(
            navArgument("fileId") { type = NavType.LongType },
            navArgument("itemId") { type = NavType.LongType },
            navArgument("startS") { type = NavType.StringType },
            navArgument("tap") { type = NavType.LongType; defaultValue = 0L },
        ),
    ) { entry ->
        val a = PlayerArgs.from(entry.arguments)
        PlayerScreen(fileId = a.fileId, itemId = a.itemId, startS = a.startS, tapMs = entry.arguments?.getLong("tap") ?: 0L)
    }
    // IPTV movies and series (#193), for users with VOD access; entered from the Library tab.
    vodGraph(user)
}

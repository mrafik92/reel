package app.reel.android.ui.shell

import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavType
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import app.reel.android.data.Me
import app.reel.android.ui.home.HomeScreen
import app.reel.android.ui.item.ItemScreen
import app.reel.android.ui.library.LibraryScreen
import app.reel.android.ui.search.SearchScreen
import app.reel.android.ui.stubs.LivePlaceholder
import app.reel.android.ui.stubs.PlayerPlaceholder
import app.reel.android.ui.stubs.SettingsPlaceholder

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
    composable(TopLevel.Settings.route) { SettingsPlaceholder(user) }

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
        ),
    ) { entry ->
        // #190 replaces the placeholder with the player screen; the arguments stay as they are.
        PlayerPlaceholder(PlayerArgs.from(entry.arguments))
    }
}

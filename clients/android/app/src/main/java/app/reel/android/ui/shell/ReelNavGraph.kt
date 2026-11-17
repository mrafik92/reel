package app.reel.android.ui.shell

import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavType
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import app.reel.android.data.Me
import app.reel.android.player.PlayerScreen
import app.reel.android.ui.vod.vodGraph
import app.reel.android.ui.home.HomeScreen
import app.reel.android.ui.stubs.ItemPlaceholder
import app.reel.android.ui.stubs.LibraryPlaceholder
import app.reel.android.ui.stubs.LivePlaceholder
import app.reel.android.ui.stubs.SearchPlaceholder
import app.reel.android.ui.stubs.SettingsPlaceholder

/**
 * The destination registry: every screen of the app is registered here. Screen tickets replace a
 * placeholder with their screen (#188 Home/Library/item, #189 Search, #191 Live TV, #194 Settings)
 * and add their own routes (player, season, …) next to these.
 */
fun NavGraphBuilder.reelGraph(user: Me) {
    composable(TopLevel.Home.route) { HomeScreen() }
    composable(TopLevel.Search.route) { SearchPlaceholder() }
    if (user.canWatchLive) composable(TopLevel.Live.route) { LivePlaceholder() }
    composable(TopLevel.Library.route) { LibraryPlaceholder(user) }
    composable(TopLevel.Settings.route) { SettingsPlaceholder(user) }

    composable(Routes.ITEM, arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
        ItemPlaceholder(entry.arguments?.getLong("id") ?: 0)
    }
    composable(
        Routes.PLAYER,
        arguments = listOf(
            navArgument("fileId") { type = NavType.LongType },
            navArgument("itemId") {
                type = NavType.LongType
                defaultValue = 0L
            },
            navArgument("startS") {
                type = NavType.FloatType
                defaultValue = 0f
            },
        ),
    ) { entry ->
        val a = entry.arguments
        PlayerScreen(
            fileId = a?.getLong("fileId") ?: 0,
            itemId = a?.getLong("itemId") ?: 0,
            startS = (a?.getFloat("startS") ?: 0f).toDouble(),
        )
    }
    // IPTV movies and series (#193), for users with VOD access; entered from the Library tab.
    vodGraph(user)
}

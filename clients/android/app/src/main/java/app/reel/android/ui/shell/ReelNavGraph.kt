package app.reel.android.ui.shell

import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavType
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import app.reel.android.data.Me
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
    composable(TopLevel.Library.route) { LibraryPlaceholder() }
    composable(TopLevel.Settings.route) { SettingsPlaceholder(user) }

    composable(Routes.ITEM, arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
        ItemPlaceholder(entry.arguments?.getLong("id") ?: 0)
    }
}

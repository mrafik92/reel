package app.reel.android.ui.shell

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarDefaults
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.foundation.focusGroup
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.navigation.NavDestination.Companion.hierarchy
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.R
import app.reel.android.data.Me
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard

/**
 * The signed-in app: a left nav rail on TV, a bottom bar on phones, and the [reelGraph] screens.
 *
 * Back: from a detail page it returns to where you were; from a top-level destination other than
 * Home it goes to Home; from Home it leaves the app.
 */
@Composable
fun AppShell(user: Me) {
    val nav = rememberNavController()
    val tabs = remember(user) { TopLevel.visibleTo(user) }
    val entry by nav.currentBackStackEntryAsState()
    val current = tabs.firstOrNull { tab -> entry?.destination?.hierarchy?.any { it.route == tab.route } == true }
    val go: (TopLevel) -> Unit = { tab -> nav.navigateTopLevel(tab) }
    // The player takes the whole screen. The rail / bar are left out (not the layout swapped), so
    // the NavHost keeps its place in the tree and its screens' state.
    val fullScreen = Routes.isFullScreen(entry?.destination?.route)

    CompositionLocalProvider(LocalNavController provides nav) {
        if (LocalDeviceKind.current == DeviceKind.Tv) {
            Row(Modifier.fillMaxSize().background(ReelColors.Bg)) {
                if (!fullScreen) TvRail(tabs, current, go)
                Box(Modifier.weight(1f).fillMaxHeight()) { ReelNavHost(nav, user) }
            }
        } else {
            Scaffold(
                containerColor = ReelColors.Bg,
                bottomBar = { if (!fullScreen) PhoneBar(tabs, current, go) },
            ) { padding ->
                Box(Modifier.padding(if (fullScreen) PaddingValues(0.dp) else padding).fillMaxSize()) { ReelNavHost(nav, user) }
            }
        }
    }
}

/** Switch tabs the standard way: one copy of each, Home at the bottom, each tab keeps its state. */
fun NavHostController.navigateTopLevel(tab: TopLevel) {
    navigate(tab.route) {
        popUpTo(graph.findStartDestination().id) { saveState = true }
        launchSingleTop = true
        restoreState = true
    }
}

@Composable
private fun ReelNavHost(nav: NavHostController, user: Me) {
    NavHost(nav, startDestination = TopLevel.Home.route, modifier = Modifier.fillMaxSize()) {
        reelGraph(user)
    }
}

/**
 * TV rail. Entering it from the content (D-pad left) lands on the current destination, not on
 * whichever item happens to be level with the focused card.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
private fun TvRail(tabs: List<TopLevel>, current: TopLevel?, go: (TopLevel) -> Unit) {
    val requesters = remember(tabs) { tabs.associateWith { FocusRequester() } }
    Column(
        Modifier
            .fillMaxHeight()
            .width(200.dp)
            .background(ReelColors.Bg1)
            .padding(vertical = 32.dp, horizontal = 16.dp)
            .focusProperties { enter = { requesters[current] ?: FocusRequester.Default } }
            .focusGroup()
            .testTag("nav-rail"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Image(
            painterResource(R.drawable.logo),
            contentDescription = "Reel",
            modifier = Modifier.size(48.dp).padding(start = 8.dp),
        )
        Spacer(Modifier.size(24.dp))
        for (tab in tabs) {
            val selected = tab == current
            FocusCard(
                onClick = { go(tab) },
                shape = RoundedCornerShape(24.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .focusRequester(requesters.getValue(tab))
                    .testTag("rail-${tab.route}"),
            ) { focused ->
                Row(
                    Modifier
                        .fillMaxWidth()
                        .background(
                            when {
                                focused -> ReelColors.Ink
                                selected -> ReelColors.AccentWash
                                else -> ReelColors.Bg1
                            },
                        )
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    val tint = when {
                        focused -> ReelColors.Bg
                        selected -> ReelColors.AccentHi
                        else -> ReelColors.Ink2
                    }
                    Icon(tab.icon, contentDescription = null, tint = tint)
                    Text(tab.label, style = MaterialTheme.typography.titleMedium, color = if (focused) ReelColors.Bg else ReelColors.Ink)
                }
            }
        }
    }
}

/** Phone bottom bar; draws behind the gesture/nav bar and pads itself for it. */
@Composable
private fun PhoneBar(tabs: List<TopLevel>, current: TopLevel?, go: (TopLevel) -> Unit) {
    NavigationBar(containerColor = ReelColors.Bg1, windowInsets = NavigationBarDefaults.windowInsets) {
        for (tab in tabs) {
            NavigationBarItem(
                selected = tab == current,
                onClick = { go(tab) },
                icon = { Icon(tab.icon, contentDescription = null) },
                label = { Text(tab.label, maxLines = 1) },
                modifier = Modifier.testTag("tab-${tab.route}"),
                colors = NavigationBarItemDefaults.colors(
                    selectedIconColor = ReelColors.AccentHi,
                    selectedTextColor = ReelColors.Ink,
                    indicatorColor = ReelColors.AccentWash,
                    unselectedIconColor = ReelColors.Ink3,
                    unselectedTextColor = ReelColors.Ink3,
                ),
            )
        }
    }
}

package app.reel.android.ui.shell

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.foundation.focusGroup
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
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
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.common.LocalServerAddress
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

    val server = LocalAppGraph.current.session.api?.server
    CompositionLocalProvider(LocalNavController provides nav, LocalServerAddress provides server) {
        if (LocalDeviceKind.current == DeviceKind.Tv) {
            // The rail sits over the content: icons only, widening with labels while it has focus.
            Box(Modifier.fillMaxSize().background(ReelColors.Bg)) {
                Box(Modifier.padding(start = RAIL_COLLAPSED).fillMaxSize()) { ReelNavHost(nav, user) }
                TvRail(tabs, current, go)
            }
        } else {
            Scaffold(
                containerColor = ReelColors.Bg,
                bottomBar = { PhoneBar(tabs, current, go) },
            ) { padding ->
                Box(Modifier.padding(padding).fillMaxSize()) { ReelNavHost(nav, user) }
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

/** Width of the TV rail when it shows icons only, and when it has focus and shows labels. */
private val RAIL_COLLAPSED = 80.dp
private val RAIL_EXPANDED = 240.dp

/**
 * TV rail: icons only until it takes focus, then it widens over the content with labels. Entering
 * it from the content (D-pad left) lands on the current destination, not on whichever item
 * happens to be level with the focused card.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
private fun TvRail(tabs: List<TopLevel>, current: TopLevel?, go: (TopLevel) -> Unit) {
    val requesters = remember(tabs) { tabs.associateWith { FocusRequester() } }
    var expanded by remember { mutableStateOf(false) }
    val width by animateDpAsState(if (expanded) RAIL_EXPANDED else RAIL_COLLAPSED, label = "rail-width")
    Column(
        Modifier
            .fillMaxHeight()
            .width(width)
            .background(
                if (expanded) {
                    Brush.horizontalGradient(0.7f to ReelColors.Bg, 1f to ReelColors.Bg.copy(alpha = 0.9f))
                } else {
                    SolidColor(ReelColors.Bg)
                },
            )
            .padding(vertical = 32.dp, horizontal = 14.dp)
            .onFocusChanged { expanded = it.hasFocus }
            .focusProperties { enter = { requesters[current] ?: FocusRequester.Default } }
            .focusGroup()
            .testTag("nav-rail"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(
            Modifier.padding(start = 6.dp, bottom = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Image(painterResource(R.drawable.logo), contentDescription = "Reel", modifier = Modifier.size(40.dp))
            if (expanded) Text("Reel", style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink, maxLines = 1)
        }
        for (tab in tabs) {
            val selected = tab == current
            FocusCard(
                onClick = { go(tab) },
                shape = RoundedCornerShape(10.dp),
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
                                else -> Color.Transparent
                            },
                        )
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    val tint = when {
                        focused -> ReelColors.Bg
                        selected -> ReelColors.AccentHi
                        else -> ReelColors.Ink2
                    }
                    Icon(tab.icon, contentDescription = tab.label, tint = tint)
                    if (expanded) {
                        Text(
                            tab.label,
                            style = MaterialTheme.typography.titleMedium,
                            color = if (focused) ReelColors.Bg else ReelColors.Ink,
                            maxLines = 1,
                        )
                    }
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

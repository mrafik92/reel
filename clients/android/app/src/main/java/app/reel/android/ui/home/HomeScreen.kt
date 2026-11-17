package app.reel.android.ui.home

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.data.Home
import app.reel.android.data.ItemSummary
import app.reel.android.ui.common.LandscapeCard
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.Loadable
import app.reel.android.ui.common.PosterCard
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory

/** One horizontal row of the home page. */
private data class HomeLane(val key: String, val title: String, val items: List<ItemSummary>, val landscape: Boolean = false)

private fun lanes(h: Home): List<HomeLane> = buildList {
    if (h.continueWatching.isNotEmpty()) add(HomeLane("continue", "Continue watching", h.continueWatching, landscape = true))
    if (h.recentlyAdded.isNotEmpty()) add(HomeLane("recent", "Recently added", h.recentlyAdded))
    h.rows.filter { it.items.isNotEmpty() }.forEach { add(HomeLane("lib-${it.libraryId}", it.title, it.items)) }
}

/**
 * Home: Continue watching, Recently added and one row per library, from `GET /home`. A first
 * version that exercises the shell's focus kit; #188 grows it (hero, mark played, …).
 */
@Composable
fun HomeScreen() {
    val home = rememberLoad { get<Home>("/home") }
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = home.state is Load.Ready)
    Loadable(home, isEmpty = { lanes(it).isEmpty() }, emptyText = "Nothing here yet. Add a library in Reel on the web.") { h ->
        HomeLanes(lanes(h), memory)
    }
}

@Composable
private fun HomeLanes(lanes: List<HomeLane>, memory: FocusMemory) {
    val nav = LocalNavController.current
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 48.dp else 16.dp
    LazyColumn(
        Modifier.fillMaxSize().testTag("home"),
        contentPadding = PaddingValues(vertical = if (tv) 32.dp else 16.dp),
        verticalArrangement = Arrangement.spacedBy(if (tv) 32.dp else 24.dp),
    ) {
        items(lanes, key = { it.key }) { lane ->
            Column {
                Text(
                    lane.title,
                    style = MaterialTheme.typography.titleMedium,
                    color = ReelColors.Ink,
                    modifier = Modifier.padding(start = edge, bottom = 12.dp),
                )
                // rememberLazyListState is saveable: the row keeps its scroll position across Back.
                LazyRow(
                    state = rememberLazyListState(),
                    contentPadding = PaddingValues(horizontal = edge, vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 12.dp),
                ) {
                    items(lane.items, key = { it.id }) { item ->
                        val m = Modifier.focusMemory(memory, "${lane.key}/${item.id}")
                        val open = { nav.navigate(Routes.item(item.id)) }
                        if (lane.landscape) {
                            LandscapeCard(item.title, item.backdropUrl ?: item.posterUrl, open, m, subtitle = episodeLabel(item), progress = item.progress)
                        } else {
                            PosterCard(item.title, item.posterUrl, open, m, subtitle = item.year?.toString(), progress = item.progress)
                        }
                    }
                }
            }
        }
    }
}

private fun episodeLabel(i: ItemSummary): String? =
    if (i.seasonNo != null && i.episodeNo != null) "S${i.seasonNo} · E${i.episodeNo}" else i.year?.toString()

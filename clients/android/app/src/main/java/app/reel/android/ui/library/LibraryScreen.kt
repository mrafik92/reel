package app.reel.android.ui.library

import app.reel.android.ui.vod.VodRoutes
import app.reel.android.data.AuthState
import androidx.compose.material3.Text
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.outlined.LiveTv
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Sort
import androidx.compose.material.icons.outlined.Category
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.GenreCount
import app.reel.android.data.ItemSummary
import app.reel.android.data.Library
import app.reel.android.data.LibraryQuery
import app.reel.android.data.LibrarySort
import app.reel.android.ui.common.CardSize
import app.reel.android.ui.common.ChoiceDialog
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.common.FooterBox
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.LoadingView
import app.reel.android.ui.common.Pill
import app.reel.android.ui.common.PosterCard
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.flow.distinctUntilChanged

/**
 * Library tab (#188): library switcher, sort / genre / unwatched filters and a paged poster grid.
 * [initialLibrary] opens a specific library (Home's *See all*).
 */
@Composable
fun LibraryScreen(initialLibrary: Long? = null) {
    val session = LocalAppGraph.current.session
    val api = session.api ?: return
    // IPTV movies & series (#193) are entered from here, for users with VOD access.
    val auth by session.state.collectAsState()
    val nav = LocalNavController.current
    val onIptv: (() -> Unit)? = if ((auth as? AuthState.SignedIn)?.user?.canWatchVod == true) {
        { nav.navigate(VodRoutes.browse()) }
    } else {
        null
    }
    val vm: LibraryViewModel = viewModel(key = "library-${initialLibrary ?: "tab"}") { LibraryViewModel(api, initialLibrary) }
    val libs by vm.libraries.collectAsState()
    when (val l = libs) {
        Load.Loading -> LoadingView()
        is Load.Failed -> ErrorView(l.message, onRetry = vm::loadLibraries)
        is Load.Ready -> if (l.value.isEmpty()) {
            if (onIptv != null) {
                Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    Text("No libraries yet. Add one in Reel on the web.", color = ReelColors.Ink2)
                    Pill("IPTV movies & series", onClick = onIptv, icon = Icons.Outlined.LiveTv, modifier = Modifier.testTag("open-vod"))
                }
            } else {
                EmptyView("No libraries yet. Add one in Reel on the web.")
            }
        } else {
            val selected by vm.selected.collectAsState()
            val query by vm.query.collectAsState()
            val genres by vm.genres.collectAsState()
            val paged by vm.pager.state.collectAsState()
            LibraryContent(
                libraries = l.value,
                selected = selected,
                query = query,
                genres = genres,
                paged = paged,
                onSelect = vm::select,
                onQuery = vm::setQuery,
                onLoadMore = vm.pager::loadMore,
                onRetry = vm.pager::retry,
                onOpen = { nav.navigate(Routes.item(it.id)) },
                onIptv = onIptv,
            )
        }
    }
}

/** Items this close to the end of the grid load the next page. */
private const val PREFETCH = 12

/** The stateless library page; [LibraryScreen] feeds it from [LibraryViewModel]. */
@Composable
fun LibraryContent(
    libraries: List<Library>,
    selected: Long?,
    query: LibraryQuery,
    genres: List<GenreCount>,
    paged: Paged,
    onSelect: (Long) -> Unit,
    onQuery: (LibraryQuery) -> Unit,
    onLoadMore: () -> Unit,
    onRetry: () -> Unit,
    onOpen: (ItemSummary) -> Unit,
    grid: LazyGridState = rememberLazyGridState(),
    /** Shown as an *IPTV* pill at the end of the filters (users with VOD access, #193). */
    onIptv: (() -> Unit)? = null,
) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 48.dp else 16.dp
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = paged.items.isNotEmpty() || paged.end || paged.error != null)

    var dialog by rememberSaveable { mutableStateOf<String?>(null) }

    // Load the next page as the grid (scrolled by touch or by D-pad focus) nears its end.
    val total by rememberUpdatedState(paged.items.size)
    val more by rememberUpdatedState(onLoadMore)
    LaunchedEffect(grid) {
        snapshotFlow { grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1 }
            .distinctUntilChanged()
            .collect { last -> if (ItemPager.nearEnd(last, total, PREFETCH)) more() }
    }
    // A short first page (or a page that didn't fill the screen) can't trigger a scroll: ask again.
    LaunchedEffect(paged.items.size, paged.loading) {
        val last = grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1
        if (!paged.loading && paged.items.isNotEmpty() && ItemPager.nearEnd(last, total, PREFETCH)) more()
    }

    Column(Modifier.fillMaxSize().testTag("library")) {
        FilterBar(libraries, selected, query, genres, memory, edge, onSelect, onQuery, onIptv) { dialog = it }
        // Until the first items arrive, show the state on its own: a grid holding only the keyed
        // footer would anchor its scroll position on it and jump to the end when the page lands.
        if (paged.items.isEmpty()) {
            when {
                paged.error != null -> ErrorView(Load.Failed(paged.error).message, onRetry = onRetry)
                paged.end -> EmptyView(if (query.filtered) "Nothing matches these filters." else "This library is empty.")
                else -> LoadingView()
            }
        } else LazyVerticalGrid(
            columns = GridCells.Adaptive(CardSize.poster()),
            state = grid,
            modifier = Modifier.fillMaxSize().testTag("library-grid"),
            contentPadding = PaddingValues(start = edge, end = edge, top = 8.dp, bottom = 32.dp),
            horizontalArrangement = Arrangement.spacedBy(if (tv) 24.dp else 12.dp),
            verticalArrangement = Arrangement.spacedBy(if (tv) 28.dp else 16.dp),
        ) {
            items(paged.items, key = { it.id }) { item ->
                PosterCard(
                    item.title,
                    item.posterUrl,
                    onClick = { onOpen(item) },
                    modifier = Modifier.fillMaxWidth().focusMemory(memory, "item/${item.id}").testTag("grid-${item.id}"),
                    subtitle = item.year?.toString(),
                    progress = item.progress,
                    played = item.played,
                )
            }
            item(key = "footer", span = { GridItemSpan(maxLineSpan) }) {
                when {
                    paged.error != null -> ErrorView(Load.Failed(paged.error).message, onRetry = onRetry, modifier = Modifier.fillMaxWidth())
                    paged.loading -> FooterBox { CircularProgressIndicator(color = ReelColors.Accent) }
                    else -> Unit
                }
            }
        }
    }

    when (dialog) {
        "sort" -> ChoiceDialog(
            title = "Sort by",
            options = LibrarySort.entries,
            selected = query.sort,
            label = { it.label },
            onPick = { onQuery(query.copy(sort = it)); dialog = null },
            onDismiss = { dialog = null },
        )
        "genre" -> ChoiceDialog(
            title = "Genre",
            options = listOf<String?>(null) + genres.map { it.name },
            selected = query.genre,
            label = { it ?: "All genres" },
            onPick = { onQuery(query.copy(genre = it)); dialog = null },
            onDismiss = { dialog = null },
        )
    }
}

@Composable
private fun FilterBar(
    libraries: List<Library>,
    selected: Long?,
    query: LibraryQuery,
    genres: List<GenreCount>,
    memory: FocusMemory,
    edge: androidx.compose.ui.unit.Dp,
    onSelect: (Long) -> Unit,
    onQuery: (LibraryQuery) -> Unit,
    onIptv: (() -> Unit)?,
    openDialog: (String) -> Unit,
) {
    // One row: libraries first (when there's more than one), then the filters. It scrolls sideways
    // on narrow phones; on TV, focus moving right scrolls it.
    LazyRow(
        state = rememberLazyListState(),
        modifier = Modifier.fillMaxWidth().testTag("filter-bar"),
        contentPadding = PaddingValues(horizontal = edge, vertical = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (libraries.size > 1) {
            items(libraries, key = { "lib-${it.id}" }) { lib ->
                Pill(
                    lib.name,
                    onClick = { onSelect(lib.id) },
                    selected = lib.id == selected,
                    modifier = Modifier.focusMemory(memory, "lib-${lib.id}").testTag("lib-${lib.id}"),
                )
            }
        }
        item(key = "sort") {
            Pill(
                query.sort.label,
                icon = Icons.AutoMirrored.Outlined.Sort,
                onClick = { openDialog("sort") },
                modifier = Modifier.focusMemory(memory, "sort").testTag("sort"),
            )
        }
        if (genres.isNotEmpty() || query.genre != null) {
            item(key = "genre") {
                Pill(
                    query.genre ?: "All genres",
                    icon = Icons.Outlined.Category,
                    selected = query.genre != null,
                    onClick = { openDialog("genre") },
                    modifier = Modifier.focusMemory(memory, "genre").testTag("genre"),
                )
            }
        }
        item(key = "unwatched") {
            Pill(
                "Unwatched",
                icon = Icons.Outlined.VisibilityOff,
                selected = query.unwatched,
                onClick = { onQuery(query.copy(unwatched = !query.unwatched)) },
                modifier = Modifier.focusMemory(memory, "unwatched").testTag("unwatched"),
            )
        }
        if (onIptv != null) {
            item(key = "iptv") {
                Pill("IPTV", icon = Icons.Outlined.LiveTv, onClick = onIptv, modifier = Modifier.focusMemory(memory, "iptv").testTag("open-vod"))
            }
        }
    }
}

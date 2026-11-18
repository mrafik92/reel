package app.reel.android.ui.vod

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.LocalBringIntoViewSpec
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ReelError
import app.reel.android.data.VodCard
import app.reel.android.data.VodEpisode
import app.reel.android.data.VodLane
import app.reel.android.data.VodMovieDetail
import app.reel.android.data.VodPage
import app.reel.android.data.VodSeriesDetail
import app.reel.android.ui.common.CardSize
import app.reel.android.ui.common.DetailActions
import app.reel.android.ui.common.DetailHeader
import app.reel.android.ui.common.Pill
import app.reel.android.ui.common.WatchProgress
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.item.ItemLogic
import app.reel.android.ui.tv.KeepInView
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.Loadable
import app.reel.android.ui.common.PosterCard
import app.reel.android.ui.common.ReelImage
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.flow.distinctUntilChanged

// IPTV movies and series (#193): browse (lanes + a full grid), movie and series pages.

private fun plural(type: String) = if (type == "series") "series" else "movies"

@Composable
private fun edge() = if (LocalDeviceKind.current == DeviceKind.Tv) 48.dp else 16.dp

/** A small pill button (Movies / Series switch, See all, seasons). */
@Composable
fun VodChip(text: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val shape = RoundedCornerShape(50)
    FocusCard(onClick = onClick, shape = shape, modifier = modifier) { focused ->
        Text(
            text,
            style = MaterialTheme.typography.labelLarge,
            color = when {
                focused -> ReelColors.Bg
                selected -> ReelColors.AccentInk
                else -> ReelColors.Ink
            },
            modifier = Modifier
                .background(
                    when {
                        focused -> ReelColors.Ink
                        selected -> ReelColors.Accent
                        else -> ReelColors.Ink.copy(alpha = 0.1f)
                    },
                    shape,
                )
                .padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
}

private fun openCard(nav: androidx.navigation.NavController, c: VodCard) =
    nav.navigate(if (c.kind == "series") VodRoutes.series(c.id) else VodRoutes.movie(c.id))

/** IPTV home: Movies / Series, curated lanes, and *All* (the paged grid). */
@Composable
fun VodBrowseScreen(type: String) {
    val nav = LocalNavController.current
    val lanes = rememberLoad(type) { get<List<VodLane>>("/vod/lanes", mapOf("type" to type)) }
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = lanes.state !is Load.Loading)
    Column(Modifier.fillMaxSize().testTag("vod-browse")) {
        Row(
            Modifier.padding(start = edge(), end = edge(), top = 24.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("IPTV", style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink, modifier = Modifier.padding(end = 12.dp))
            for ((t, label) in listOf("movie" to "Movies", "series" to "Series")) {
                VodChip(label, selected = t == type, onClick = {
                    if (t != type) nav.navigate(VodRoutes.browse(t)) { popUpTo(VodRoutes.BROWSE) { inclusive = true } }
                }, modifier = Modifier.focusMemory(memory, "tab-$t").testTag("vod-tab-$t"))
            }
            VodChip(
                if (type == "series") "All series" else "All movies",
                selected = false,
                onClick = { nav.navigate(VodRoutes.grid(type)) },
                modifier = Modifier.focusMemory(memory, "all").testTag("vod-all"),
            )
        }
        val s = lanes.state
        if (s is Load.Ready && s.value.none { it.items.isNotEmpty() }) {
            // No lanes yet (details not loaded from the provider): the whole catalogue instead.
            VodGrid(type, "", Modifier.weight(1f))
        } else {
            Loadable(lanes) { ls -> VodLanes(ls.filter { it.items.isNotEmpty() }, type, memory) }
        }
    }
}

@Composable
private fun VodLanes(lanes: List<VodLane>, type: String, memory: FocusMemory) {
    val nav = LocalNavController.current
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(if (tv) 28.dp else 20.dp),
    ) {
        items(lanes, key = { it.key }) { lane ->
            Column {
                Row(Modifier.padding(start = edge(), end = edge(), bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(lane.title, style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink)
                        lane.subtitle?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3) }
                    }
                    lane.seeAll?.let { sa ->
                        VodChip("See all", false, { nav.navigate(VodRoutes.grid(sa.type.ifEmpty { type }, sa.query)) }, Modifier.focusMemory(memory, "${lane.key}/all"))
                    }
                }
                LazyRow(
                    state = rememberLazyListState(),
                    contentPadding = PaddingValues(horizontal = edge(), vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 12.dp),
                ) {
                    items(lane.items, key = { "${it.kind}-${it.id}" }) { c ->
                        PosterCard(
                            c.title,
                            c.posterUrl,
                            { openCard(nav, c) },
                            Modifier.focusMemory(memory, "${lane.key}/${c.id}"),
                            subtitle = c.year?.toString(),
                            progress = c.progress,
                        )
                    }
                }
            }
        }
    }
}

/** The whole catalogue (or a lane's *see all*), paged with the server's cursor. */
@Composable
fun VodGridScreen(type: String, query: String) {
    Column(Modifier.fillMaxSize()) {
        Text(
            if (type == "series") "IPTV series" else "IPTV movies",
            style = MaterialTheme.typography.headlineSmall,
            color = ReelColors.Ink,
            modifier = Modifier.padding(start = edge(), top = 24.dp, bottom = 8.dp),
        )
        VodGrid(type, query, Modifier.weight(1f))
    }
}

/** `a=1&b=2` → map (values decoded). */
internal fun parseQuery(q: String): Map<String, String> = q.trimStart('?').split('&').filter { it.isNotEmpty() }.associate { kv ->
    val k = kv.substringBefore('=')
    android.net.Uri.decode(k) to android.net.Uri.decode(kv.substringAfter('=', ""))
}

@Composable
private fun VodGrid(type: String, query: String, modifier: Modifier) {
    val api = LocalAppGraph.current.session.api ?: return
    val nav = LocalNavController.current
    val items = remember(type, query) { mutableStateListOf<VodCard>() }
    var cursor by remember(type, query) { mutableStateOf<String?>(null) }
    var done by remember(type, query) { mutableStateOf(false) }
    var loading by remember(type, query) { mutableStateOf(false) }
    var error by remember(type, query) { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    val grid = rememberLazyGridState()
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = items.isNotEmpty())

    suspend fun more() {
        if (loading || done) return
        loading = true
        error = null
        try {
            val page = api.get<VodPage>("/vod/${plural(type)}", parseQuery(query) + mapOf("cursor" to cursor, "limit" to 60))
            items += page.items.filter { n -> items.none { it.id == n.id } }
            cursor = page.nextCursor
            done = page.nextCursor == null
        } catch (e: ReelError) {
            error = e.userMessage
        } finally {
            loading = false
        }
    }
    LaunchedEffect(type, query, attempt) { if (items.isEmpty()) more() }
    // Load the next page when the last rows come into view.
    LaunchedEffect(grid, type, query) {
        snapshotFlow { grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }
            .distinctUntilChanged()
            .collect { last -> if (last >= items.size - 12 && items.isNotEmpty()) more() }
    }
    Box(modifier.fillMaxSize()) {
        when {
            items.isEmpty() && error != null -> ErrorView(error!!, onRetry = { attempt++ })
            items.isEmpty() && loading -> CircularProgressIndicator(Modifier.align(Alignment.Center), color = ReelColors.Accent)
            items.isEmpty() && done -> Text("Nothing here.", color = ReelColors.Ink3, modifier = Modifier.align(Alignment.Center))
            else -> LazyVerticalGrid(
                GridCells.Adaptive(CardSize.poster() + 8.dp),
                state = grid,
                contentPadding = PaddingValues(horizontal = edge(), vertical = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
                modifier = Modifier.fillMaxSize().testTag("vod-grid"),
            ) {
                items(items, key = { "${it.kind}-${it.id}" }) { c ->
                    PosterCard(c.title, c.posterUrl, { openCard(nav, c) }, Modifier.focusMemory(memory, "c${c.id}"), subtitle = c.year?.toString(), progress = c.progress)
                }
            }
        }
    }
}

@Composable
private fun VodSynopsis(overview: String?, credits: String) {
    Column(Modifier.padding(horizontal = edge()).widthIn(max = 800.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        overview?.takeIf { it.isNotBlank() }?.let {
            Text(it, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2)
        }
        if (credits.isNotBlank()) Text(credits, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
    }
}

private fun runtime(s: Long?) = s?.takeIf { it > 0 }?.let(ItemLogic::runtime)

@Composable
private fun VodActions(label: String, resume: Double, memory: FocusMemory, start: (Double) -> Unit) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    DetailActions {
        Pill(
            label, { start(resume) }, primary = true, icon = Icons.Outlined.PlayArrow,
            modifier = (if (tv) Modifier else Modifier.fillMaxWidth()).focusMemory(memory, "play").testTag("vod-play"),
        )
        if (resume > 0) Pill("Play from start", { start(0.0) }, Modifier.focusMemory(memory, "start"))
    }
}

@Composable
fun VodMovieScreen(id: Long) {
    val nav = LocalNavController.current
    val movie = rememberLoad(id) { get<VodMovieDetail>("/vod/movies/$id") }
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = movie.state is Load.Ready, first = "play")
    Loadable(movie) { m ->
        DetailList("vod-movie") {
            item(key = "hero") {
                val resume = vodResumeS(m.progress, m.runtimeS, m.played)
                DetailHeader(
                    m.title, "IPTV film", m.posterUrl, m.backdropUrl,
                    listOfNotNull(ItemLogic.stars(m.rating), m.year?.toString(), runtime(m.runtimeS)) + m.genres.orEmpty().take(3),
                ) {
                    VodActions(ItemLogic.playLabel(resume, remaining = ItemLogic.timeLeft(m.progress, m.runtimeS?.toDouble())), resume, memory) {
                        nav.navigate(VodRoutes.player("movie", m.id, null, it))
                    }
                }
            }
            item(key = "synopsis") {
                val credits = listOfNotNull(m.director?.let { "Director: $it" }, m.cast.take(5).joinToString(", ") { it.name }.ifEmpty { null })
                VodSynopsis(m.overview, credits.joinToString(" · "))
            }
        }
    }
}

@Composable
fun VodSeriesScreen(id: Long) {
    val nav = LocalNavController.current
    val series = rememberLoad(id) { get<VodSeriesDetail>("/vod/series/$id") }
    val memory = rememberFocusMemory()
    var seasonNo by rememberSaveable(id) { mutableStateOf<Long?>(null) }
    RestoreFocus(memory, ready = series.state is Load.Ready, first = "play")
    Loadable(series) { s ->
        val seasons = s.seasons.filter { it.episodes.isNotEmpty() }
        val current = seasons.firstOrNull { it.seasonNo == seasonNo }
            ?: seasons.firstOrNull { se -> se.episodes.any { it.played != true } } ?: seasons.firstOrNull()
        DetailList("vod-series") {
            item(key = "hero") {
                DetailHeader(
                    s.title, "IPTV series", s.posterUrl, s.backdropUrl,
                    listOfNotNull(ItemLogic.stars(s.rating), s.year?.toString(), "${seasons.size} season${if (seasons.size == 1) "" else "s"}")
                        + s.genres.orEmpty().take(3),
                ) {
                    val next = current?.let { vodNextEpisode(it.episodes) }
                    if (next != null) {
                        val resume = vodResumeS(next.progress, next.runtimeS, next.played)
                        val label = ItemLogic.playLabel(resume, ItemLogic.episodeCode(current.seasonNo, next.episodeNo))
                        VodActions(label, resume, memory) { nav.navigate(VodRoutes.player("episode", next.id, s.id, it)) }
                    }
                }
            }
            item(key = "synopsis") { VodSynopsis(s.overview, s.cast.take(5).joinToString(", ") { it.name }) }
            item(key = "episodes-heading") {
                Text("Episodes", style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink, modifier = Modifier.padding(horizontal = edge()))
            }
            if (seasons.size > 1) {
                item(key = "seasons") {
                    LazyRow(contentPadding = PaddingValues(horizontal = edge()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        items(seasons, key = { it.seasonNo }) { se ->
                            val active = se.seasonNo == current?.seasonNo
                            Pill(se.name ?: "Season ${se.seasonNo}", { seasonNo = se.seasonNo }, selected = active,
                                modifier = Modifier.focusMemory(memory, "season/${se.seasonNo}").semantics { selected = active })
                        }
                    }
                }
            }
            if (current == null) {
                item(key = "empty") { Text("No episodes yet.", color = ReelColors.Ink2, modifier = Modifier.padding(horizontal = edge())) }
            } else {
                items(current.episodes, key = { it.id }) { e ->
                    EpisodeRow(e, Modifier.focusMemory(memory, "ep/${e.id}")) {
                        nav.navigate(VodRoutes.player("episode", e.id, s.id, vodResumeS(e.progress, e.runtimeS, e.played)))
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DetailList(tag: String, content: LazyListScope.() -> Unit) {
    CompositionLocalProvider(LocalBringIntoViewSpec provides KeepInView) {
        LazyColumn(
            Modifier.fillMaxSize().testTag(tag),
            contentPadding = PaddingValues(bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
            content = content,
        )
    }
}

@Composable
private fun EpisodeRow(e: VodEpisode, modifier: Modifier, onClick: () -> Unit) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    FocusCard(onClick = onClick, shape = RoundedCornerShape(10.dp), modifier = modifier.padding(horizontal = edge()).fillMaxWidth().testTag("vod-ep-${e.id}")) { focused ->
        Row(
            Modifier.fillMaxWidth().background(if (focused) ReelColors.Bg3 else ReelColors.Bg1).padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(if (tv) 200.dp else 120.dp).aspectRatio(16f / 9f).clip(RoundedCornerShape(8.dp))) {
                ReelImage(e.stillUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
                WatchProgress(if (e.played == true) 1.0 else e.progress, Modifier.align(Alignment.BottomCenter).fillMaxWidth())
            }
            Column(Modifier.weight(1f)) {
                Text("${e.episodeNo}. ${e.title}", style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val status = if (e.played == true) "Watched" else ItemLogic.timeLeft(e.progress, e.runtimeS?.toDouble())
                val meta = listOfNotNull(runtime(e.runtimeS), status)
                if (meta.isNotEmpty()) Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
                e.overview?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
}

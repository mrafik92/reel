package app.reel.android.ui.home

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowForward
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.Home
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.home
import app.reel.android.data.item
import app.reel.android.ui.common.CardSize
import app.reel.android.ui.common.LandscapeCard
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.Loadable
import app.reel.android.ui.common.OnResume
import app.reel.android.ui.common.Pill
import app.reel.android.ui.common.PosterCard
import app.reel.android.ui.common.ReelImage
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.item.ItemLogic
import app.reel.android.ui.item.rememberPlayLauncher
import app.reel.android.ui.live.LiveNowRow
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay

/** One horizontal row of the home page. */
internal data class HomeLane(
    val key: String,
    val title: String,
    val items: List<ItemSummary>,
    val landscape: Boolean = false,
    /** Library rows end with a *See all* card that opens this library's grid. */
    val libraryId: Long? = null,
)

internal fun lanes(h: Home): List<HomeLane> = buildList {
    if (h.continueWatching.isNotEmpty()) add(HomeLane("continue", "Continue watching", h.continueWatching, landscape = true))
    if (h.recentlyAdded.isNotEmpty()) add(HomeLane("recent", "Recently added", h.recentlyAdded))
    h.rows.filter { it.items.isNotEmpty() }.forEach { add(HomeLane("lib-${it.libraryId}", it.title, it.items, libraryId = it.libraryId)) }
}

/** The title the home page features: the first one in progress, else the newest. */
internal fun featured(h: Home): ItemSummary? = h.continueWatching.firstOrNull() ?: h.recentlyAdded.firstOrNull()

/** `S2 · E5 · 24 min left`, `Drama · 2019 · 1 h left`: the line under a featured or continue card. */
internal fun cardMeta(i: ItemSummary, withGenres: Boolean = false): String? = listOfNotNull(
    ItemLogic.episodeCode(i.seasonNo, i.episodeNo),
    if (withGenres) i.genres?.take(2)?.joinToString(" · ")?.ifEmpty { null } else null,
    if (i.kind == "episode") null else i.year?.toString(),
    ItemLogic.timeLeft(i.progress, i.runtimeS?.toDouble()),
).joinToString(" · ").ifEmpty { null }

private const val EMPTY_HOME = "Nothing here yet. Add a library in Reel on the web."

/**
 * Home (#188): a featured title, then Continue watching (with progress), Recently added and one row
 * per library (ending in *See all*), from `GET /home`.
 *
 * - Phone: a large featured title with *Resume* / *Details*, then the rows.
 * - TV: the featured area at the top follows the focused card (title, rating, meta, overview).
 *
 * With Live TV access, a *Live now* row of favourite channels follows the first row (#191).
 *
 * It reloads when shown again (Back) and refreshes quietly when the app comes back to the front.
 */
@Composable
fun HomeScreen(live: Boolean) {
    val home = rememberLoad { home() }
    OnResume { home.refresh() }
    val memory = rememberFocusMemory()
    RestoreFocus(memory, ready = home.state is Load.Ready)
    Loadable(home, isEmpty = { !live && lanes(it).isEmpty() }, emptyText = EMPTY_HOME) { h ->
        if (LocalDeviceKind.current == DeviceKind.Tv) TvHome(h, memory, live) else PhoneHome(h, memory, live)
    }
}

// ---- phone ---------------------------------------------------------------------------------

@Composable
private fun PhoneHome(h: Home, memory: FocusMemory, live: Boolean) {
    val feature = featured(h)
    LazyColumn(
        Modifier.fillMaxSize().testTag("home"),
        contentPadding = PaddingValues(bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(24.dp),
    ) {
        if (feature != null) item(key = "featured") { PhoneFeatured(feature) }
        val all = lanes(h)
        items(all.take(1), key = { it.key }) { lane -> Lane(lane, memory, edge = 16.dp, tv = false) }
        if (live) item(key = "live-now") { LiveNowRow(memory, edge = 16.dp, tv = false, emptyText = EMPTY_HOME.takeIf { all.isEmpty() }) }
        items(all.drop(1), key = { it.key }) { lane -> Lane(lane, memory, edge = 16.dp, tv = false) }
    }
}

/** ~430dp featured title fading into the page, with cream *Resume* and *Details*. */
@Composable
private fun PhoneFeatured(i: ItemSummary) {
    val nav = LocalNavController.current
    val player = rememberPlayLauncher()
    val resuming = !i.played && (i.progress ?: 0.0) >= ItemLogic.STARTED
    Box(Modifier.fillMaxWidth().height(430.dp).testTag("featured")) {
        ReelImage(i.backdropUrl ?: i.posterUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(
                    0f to ReelColors.Bg.copy(alpha = 0.6f),
                    0.38f to Color.Transparent,
                    0.62f to ReelColors.Bg.copy(alpha = 0.55f),
                    1f to ReelColors.Bg,
                ),
            ),
        )
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(horizontal = 20.dp, vertical = 18.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                i.title,
                style = MaterialTheme.typography.displaySmall.copy(fontSize = 40.sp, lineHeight = 40.sp, fontWeight = FontWeight.ExtraBold),
                color = ReelColors.Ink,
                textAlign = TextAlign.Center,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            cardMeta(i, withGenres = true)?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2) }
            val p = i.progress?.toFloat()?.coerceIn(0f, 1f)
            if (resuming && p != null) {
                Box(Modifier.width(160.dp).height(3.dp).background(ReelColors.Ink.copy(alpha = 0.2f), RoundedCornerShape(2.dp))) {
                    Box(Modifier.fillMaxWidth(p).fillMaxHeight().background(ReelColors.AccentHi, RoundedCornerShape(2.dp)))
                }
            }
            Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill(
                    if (resuming) "Resume" else "Play",
                    icon = Icons.Outlined.PlayArrow,
                    primary = true,
                    enabled = i.kind == "movie" || i.kind == "episode",
                    onClick = { player.play(i.id) },
                    modifier = Modifier.width(150.dp).testTag("featured-play"),
                )
                Pill(
                    "Details",
                    icon = Icons.Outlined.Info,
                    onClick = { nav.navigate(Routes.item(i.id)) },
                    modifier = Modifier.width(150.dp).testTag("featured-details"),
                )
            }
            player.error?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.AccentHi) }
        }
    }
}

// ---- TV ------------------------------------------------------------------------------------

/** The focused card and the row it's in (the featured area's kicker). */
private data class Spotlight(val lane: HomeLane?, val item: ItemSummary)

@Composable
private fun TvHome(h: Home, memory: FocusMemory, live: Boolean) {
    val all = lanes(h)
    var spot by remember { mutableStateOf(featured(h)?.let { f -> Spotlight(all.firstOrNull { f in it.items }, f) }) }
    Column(Modifier.fillMaxSize().testTag("home")) {
        spot?.let { TvFeatured(it) }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(top = 8.dp, bottom = 48.dp),
            verticalArrangement = Arrangement.spacedBy(28.dp),
        ) {
            items(all.take(1), key = { it.key }) { lane ->
                Lane(lane, memory, edge = 32.dp, tv = true, onFocusItem = { spot = Spotlight(lane, it) })
            }
            if (live) item(key = "live-now") { LiveNowRow(memory, edge = 32.dp, tv = true, emptyText = EMPTY_HOME.takeIf { all.isEmpty() }) }
            items(all.drop(1), key = { it.key }) { lane ->
                Lane(lane, memory, edge = 32.dp, tv = true, onFocusItem = { spot = Spotlight(lane, it) })
            }
        }
    }
}

/**
 * The top of the TV home: the focused card's backdrop on the right, fading into the page, with its
 * title, rating, meta and overview (loaded once focus rests on it).
 */
@Composable
private fun TvFeatured(s: Spotlight) {
    val api = LocalAppGraph.current.session.api
    val details = remember { mutableStateMapOf<Long, ItemDetail>() }
    val i = s.item
    LaunchedEffect(i.id) {
        if (api == null || details.containsKey(i.id)) return@LaunchedEffect
        delay(350) // only once focus settles, not for every card passed on the way
        try {
            details[i.id] = api.item(i.id)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // The overview is a nicety; the card's own fields are already shown.
        }
    }
    val d = details[i.id]
    Box(Modifier.fillMaxWidth().height(270.dp).testTag("tv-featured")) {
        Box(Modifier.align(Alignment.TopEnd).fillMaxWidth(0.72f).fillMaxHeight()) {
            ReelImage(i.backdropUrl ?: i.posterUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
            Box(Modifier.fillMaxSize().background(Brush.horizontalGradient(0f to ReelColors.Bg, 0.3f to ReelColors.Bg.copy(alpha = 0.6f), 0.7f to Color.Transparent)))
            Box(Modifier.fillMaxSize().background(Brush.verticalGradient(0.55f to Color.Transparent, 1f to ReelColors.Bg)))
        }
        Column(Modifier.padding(start = 32.dp, top = 28.dp).width(560.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            val kicker = when (s.lane?.key) {
                "continue" -> listOfNotNull("Continue", ItemLogic.timeLeft(i.progress, i.runtimeS?.toDouble()) ?: ItemLogic.episodeCode(i.seasonNo, i.episodeNo))
                    .joinToString(" · ")
                null -> null
                else -> s.lane.title
            }
            kicker?.let { Text(it.uppercase(), style = MaterialTheme.typography.labelLarge, color = ReelColors.AccentHi) }
            Text(
                i.title,
                style = MaterialTheme.typography.displaySmall.copy(fontWeight = FontWeight.ExtraBold),
                color = ReelColors.Ink,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                (d?.rating ?: i.rating)?.takeIf { it > 0 }?.let {
                    Text(ItemLogic.stars(it)!!, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold), color = ReelColors.Ok)
                }
                val meta = d?.let { ItemLogic.meta(it).joinToString(" · ") } ?: cardMeta(i, withGenres = true)
                meta?.takeIf { it.isNotEmpty() }?.let { Text(it, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2, maxLines = 1) }
            }
            d?.overview?.takeIf { it.isNotBlank() }?.let {
                Text(it, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink.copy(alpha = 0.82f), maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

// ---- rows ----------------------------------------------------------------------------------

@Composable
private fun Lane(lane: HomeLane, memory: FocusMemory, edge: androidx.compose.ui.unit.Dp, tv: Boolean, onFocusItem: (ItemSummary) -> Unit = {}) {
    val nav = LocalNavController.current
    Column {
        Text(
            lane.title,
            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.ExtraBold),
            color = ReelColors.Ink,
            modifier = Modifier.padding(start = edge, bottom = 10.dp),
        )
        // rememberLazyListState is saveable: the row keeps its scroll position across Back.
        LazyRow(
            state = rememberLazyListState(),
            contentPadding = PaddingValues(horizontal = edge, vertical = if (tv) 12.dp else 4.dp),
            horizontalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 10.dp),
        ) {
            items(lane.items, key = { it.id }) { item ->
                val m = Modifier.focusMemory(memory, "${lane.key}/${item.id}").onFocusChanged { if (it.hasFocus) onFocusItem(item) }
                val open = { nav.navigate(Routes.item(item.id)) }
                if (lane.landscape) {
                    LandscapeCard(item.title, item.backdropUrl ?: item.posterUrl, open, m, subtitle = cardMeta(item), progress = item.progress)
                } else {
                    PosterCard(item.title, item.posterUrl, open, m, subtitle = item.year?.toString(), progress = item.progress, played = item.played)
                }
            }
            lane.libraryId?.let { lib ->
                item(key = "see-all") {
                    SeeAllCard(
                        onClick = { nav.navigate(Routes.library(lib)) },
                        modifier = Modifier.focusMemory(memory, "${lane.key}/all").testTag("see-all-$lib"),
                    )
                }
            }
        }
    }
}

/** Poster-sized card at the end of a library row: opens the full, paged library. */
@Composable
private fun SeeAllCard(onClick: () -> Unit, modifier: Modifier) {
    Column(modifier.width(CardSize.poster())) {
        FocusCard(onClick = onClick, modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f)) { focused ->
            Box(
                Modifier.fillMaxSize().background(if (focused) ReelColors.Ink else ReelColors.Bg2),
                contentAlignment = Alignment.Center,
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Icon(Icons.AutoMirrored.Outlined.ArrowForward, contentDescription = null, tint = if (focused) ReelColors.Bg else ReelColors.Ink)
                    Text("See all", style = MaterialTheme.typography.titleMedium, color = if (focused) ReelColors.Bg else ReelColors.Ink)
                }
            }
        }
    }
}

package app.reel.android.ui.item

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.LocalBringIntoViewSpec
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.Replay
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.CastMember
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.children
import app.reel.android.data.item
import app.reel.android.data.setPlayed
import app.reel.android.ui.common.CardSize
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.LoadHandle
import app.reel.android.ui.common.Loadable
import app.reel.android.ui.common.Pill
import app.reel.android.ui.common.ReelImage
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.KeepInView
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Item detail (#188): backdrop, poster, title, rating + metadata, file badges (4K / HDR / 5.1),
 * overview, Play / Resume / Play from start, mark watched, season tabs and episode rows (shows and
 * seasons), cast.
 *
 * The page reloads whenever it's shown again (Back from the player), so progress is fresh.
 */
@Composable
fun ItemScreen(id: Long) {
    val detail = rememberLoad(id) { item(id) }
    val memory = rememberFocusMemory()
    // Shows: which season's episodes are listed. Saved, so Back from an episode keeps it.
    var season by rememberSaveable(id) { mutableStateOf<Long?>(null) }
    Loadable(detail) { d ->
        val episodesOf: Long? = when (d.kind) {
            "season" -> d.id
            "show" -> season
            else -> null
        }
        val seasons = if (d.kind == "show") rememberLoad(d.id) { children(d.id) } else null
        val episodes = episodesOf?.let { sid -> rememberLoad(sid) { children(sid) } }
        val seasonList = (seasons?.state as? Load.Ready)?.value
        LaunchedEffect(seasonList) {
            if (season == null) season = seasonList?.let { ItemLogic.defaultSeason(it) }?.id
        }
        val ready = when (d.kind) {
            "show" -> seasons?.state !is Load.Loading && (seasonList.isNullOrEmpty() || episodes?.state is Load.Ready || episodes?.state is Load.Failed)
            "season" -> episodes?.state !is Load.Loading
            else -> true
        }
        RestoreFocus(memory, ready = ready, first = "play")
        ItemBody(d, seasonList, season, { season = it }, episodes, memory)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ItemBody(
    d: ItemDetail,
    seasons: List<ItemSummary>?,
    season: Long?,
    onSeason: (Long) -> Unit,
    episodes: LoadHandle<List<ItemSummary>>?,
    memory: FocusMemory,
) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 48.dp else 20.dp
    val epList = (episodes?.state as? Load.Ready)?.value
    CompositionLocalProvider(LocalBringIntoViewSpec provides KeepInView) { ItemList(d, seasons, season, onSeason, episodes, epList, memory, tv, edge) }
}

@Composable
private fun ItemList(
    d: ItemDetail,
    seasons: List<ItemSummary>?,
    season: Long?,
    onSeason: (Long) -> Unit,
    episodes: LoadHandle<List<ItemSummary>>?,
    epList: List<ItemSummary>?,
    memory: FocusMemory,
    tv: Boolean,
    edge: Dp,
) {
    LazyColumn(
        Modifier.fillMaxSize().testTag("item"),
        contentPadding = PaddingValues(bottom = 32.dp),
        verticalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 14.dp),
    ) {
        item(key = "hero") { Hero(d, tv, edge) { Actions(d, epList, memory, tv) } }

        if (!seasons.isNullOrEmpty() && seasons.size > 1) {
            item(key = "seasons") { SeasonTabs(seasons, season, onSeason, memory, edge) }
        } else if (d.kind == "season" || !seasons.isNullOrEmpty()) {
            item(key = "episodes-title") { SectionTitle("Episodes", edge) }
        }

        if (episodes != null) episodeRows(episodes, memory, edge, tv)

        if (d.cast.isNotEmpty()) {
            item(key = "cast") {
                Column(Modifier.padding(top = 12.dp)) {
                    SectionTitle("Cast", edge)
                    LazyRow(
                        state = rememberLazyListState(),
                        contentPadding = PaddingValues(horizontal = edge, vertical = 8.dp),
                        horizontalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        itemsIndexed(d.cast.take(30)) { i, c ->
                            CastCard(c, Modifier.focusMemory(memory, "cast/$i"))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SectionTitle(title: String, edge: Dp) {
    Text(
        title,
        style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.ExtraBold),
        color = ReelColors.Ink,
        modifier = Modifier.padding(start = edge, end = edge, top = 8.dp),
    )
}

/** Backdrop fading into the page, then poster (TV) + title + rating/meta + badges + [actions]. */
@Composable
private fun Hero(d: ItemDetail, tv: Boolean, edge: Dp, actions: @Composable () -> Unit) {
    val fade = Brush.verticalGradient(0f to ReelColors.Bg.copy(alpha = 0.5f), 0.45f to Color.Transparent, 1f to ReelColors.Bg)
    val backdrop = d.backdropUrl ?: d.posterUrl
    if (tv) {
        Box(Modifier.fillMaxWidth()) {
            ReelImage(backdrop, contentDescription = null, modifier = Modifier.fillMaxWidth().height(400.dp))
            Box(Modifier.matchParentSize().background(Brush.horizontalGradient(0f to ReelColors.Bg, 0.6f to ReelColors.Bg.copy(alpha = 0.3f), 1f to Color.Transparent)))
            Box(Modifier.matchParentSize().background(fade))
            Row(Modifier.padding(start = edge, end = edge, top = 48.dp), horizontalArrangement = Arrangement.spacedBy(32.dp)) {
                if (d.kind != "episode" && d.posterUrl != null) {
                    ReelImage(d.posterUrl, contentDescription = d.title, modifier = Modifier.width(220.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(10.dp)))
                }
                Column(Modifier.weight(1f).padding(top = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    TitleBlock(d, tv = true)
                    Overview(d, maxLines = 4)
                    // Pulled left by the action row's padding so the first button lines up with the title.
                    Box(Modifier.padding(top = 4.dp).offset(x = (-12).dp)) { actions() }
                }
            }
        }
    } else {
        Box(Modifier.fillMaxWidth().height(250.dp)) {
            ReelImage(backdrop, contentDescription = null, modifier = Modifier.fillMaxSize())
            Box(Modifier.fillMaxSize().background(fade))
        }
        Column(
            Modifier.padding(start = edge, end = edge),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            TitleBlock(d, tv = false)
            actions()
            Overview(d, maxLines = Int.MAX_VALUE)
        }
    }
}

@Composable
private fun TitleBlock(d: ItemDetail, tv: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(ItemLogic.kindLabel(d).uppercase(), style = MaterialTheme.typography.labelLarge, color = ReelColors.AccentHi)
        Text(
            d.title,
            style = MaterialTheme.typography.displaySmall.copy(
                fontWeight = FontWeight.ExtraBold,
                fontSize = if (tv) 52.sp else 32.sp,
                lineHeight = if (tv) 54.sp else 34.sp,
            ),
            color = ReelColors.Ink,
            modifier = Modifier.testTag("item-title"),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            ItemLogic.stars(d.rating)?.let { Text(it, style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Bold), color = ReelColors.Ok) }
            val meta = ItemLogic.meta(d)
            if (meta.isNotEmpty()) Text(meta.joinToString("  ·  "), style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2)
        }
        val badges = ItemLogic.badges(d.files.firstOrNull())
        if (badges.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                for (b in badges) Badge(b)
            }
        }
    }
}

@Composable
private fun Badge(text: String) {
    Box(
        Modifier
            .heightIn(min = 26.dp)
            .border(1.dp, ReelColors.Ink.copy(alpha = 0.24f), RoundedCornerShape(6.dp))
            .padding(horizontal = 10.dp, vertical = 4.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold, fontSize = 12.sp), color = ReelColors.Ink)
    }
}

@Composable
private fun Overview(d: ItemDetail, maxLines: Int) {
    val text = d.overview?.takeIf { it.isNotBlank() } ?: return
    Text(
        text,
        style = MaterialTheme.typography.bodyLarge,
        color = ReelColors.Ink2,
        maxLines = maxLines,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier.widthIn(max = 760.dp),
    )
}

/** What the main button plays: the item itself, or a show's / season's next episode. */
private data class PlayTarget(val label: String, val startS: Double?, val play: () -> Unit, val restart: (() -> Unit)?)

/**
 * The main button (cream, full width on phones) names what it plays and the time left
 * (`Resume S2 · E3 · 18 min left`, `Resume from 38:13 · 52 min left`). Then *Play from start* (when
 * resuming) and *Mark watched*. Shows and seasons play their next episode.
 */
@Composable
private fun Actions(d: ItemDetail, episodes: List<ItemSummary>?, memory: FocusMemory, tv: Boolean) {
    val api = LocalAppGraph.current.session.api
    val nav = LocalNavController.current
    val scope = rememberCoroutineScope()
    val player = rememberPlayLauncher()
    var played by remember(d.id, d.played) { mutableStateOf(d.played) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    val target: PlayTarget? = if (d.kind == "show" || d.kind == "season") {
        episodes?.let { ItemLogic.nextUp(it) }?.let { next ->
            val resuming = !next.played && (next.progress ?: 0.0) >= ItemLogic.STARTED
            val code = ItemLogic.episodeCode(next.seasonNo, next.episodeNo) ?: next.title
            val left = if (resuming) ItemLogic.timeLeft(next.progress, next.runtimeS?.toDouble()) else null
            PlayTarget(
                listOfNotNull("${if (resuming) "Resume" else "Play"} $code", left).joinToString(" · "),
                null,
                { player.play(next.id) },
                if (resuming) ({ player.play(next.id, fromStart = true) }) else null,
            )
        }
    } else {
        d.files.firstOrNull()?.let { file ->
            val resume = ItemLogic.resumeAt(d.progress, file.durationS, played)
            val left = if (resume > 0) ItemLogic.timeLeft(d.progress, file.durationS) else null
            PlayTarget(
                if (resume > 0) listOfNotNull("Resume from ${ItemLogic.clock(resume)}", left).joinToString(" · ") else "Play",
                resume,
                { nav.navigate(Routes.player(file.id, d.id, resume)) },
                if (resume > 0) ({ nav.navigate(Routes.player(file.id, d.id, 0.0)) }) else null,
            )
        }
    }

    val togglePlayed = {
        if (!saving && api != null) {
            val next = !played
            played = next
            saving = true
            error = null
            scope.launch {
                try {
                    api.setPlayed(d.id, next)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    played = !next
                    error = Load.Failed(e).message
                } finally {
                    saving = false
                }
            }
        }
    }
    val watchedLabel = if (played) "Mark unwatched" else "Mark watched"
    val watchedIcon = if (played) Icons.Outlined.VisibilityOff else Icons.Outlined.CheckCircle

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (tv) {
            // At most three buttons: a plain row (a lazy one scrolls sideways when focus is restored).
            // Its padding leaves room for a focused pill's 1.1x scale, so the edge isn't clipped.
            Row(
                Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                target?.let { t ->
                    Pill(t.label, icon = Icons.Outlined.PlayArrow, primary = true, onClick = t.play, modifier = Modifier.focusMemory(memory, "play").testTag("play"))
                    t.restart?.let { r ->
                        Pill("Play from start", icon = Icons.Outlined.Replay, onClick = r, modifier = Modifier.focusMemory(memory, "restart").testTag("restart"))
                    }
                }
                Pill(watchedLabel, icon = watchedIcon, onClick = togglePlayed, modifier = Modifier.focusMemory(memory, "played").testTag("played"))
                if (saving || player.busy) CircularProgressIndicator(color = ReelColors.Accent, modifier = Modifier.size(28.dp))
            }
        } else {
            target?.let { t ->
                WideButton(t.label, Icons.Outlined.PlayArrow, t.play, Modifier.focusMemory(memory, "play").testTag("play"))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                target?.restart?.let { r ->
                    TileButton("Start over", Icons.Outlined.Replay, r, Modifier.weight(1f).focusMemory(memory, "restart").testTag("restart"))
                }
                TileButton(if (played) "Unwatched" else "Watched", watchedIcon, togglePlayed, Modifier.weight(1f).focusMemory(memory, "played").testTag("played"))
            }
        }
        if (d.files.isEmpty() && d.kind != "show" && d.kind != "season") {
            Text("No playable file for this title yet.", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        }
        (error ?: player.error)?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.AccentHi, modifier = Modifier.testTag("item-error")) }
    }
}

/** Full-width cream button (the phone's main action). */
@Composable
private fun WideButton(text: String, icon: ImageVector, onClick: () -> Unit, modifier: Modifier) {
    FocusCard(onClick = onClick, modifier = modifier.fillMaxWidth(), shape = RoundedCornerShape(8.dp)) { focused ->
        Row(
            Modifier.fillMaxWidth().background(if (focused) ReelColors.AccentHi else ReelColors.Ink).height(48.dp).padding(horizontal = 16.dp),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val fg = if (focused) ReelColors.AccentInk else ReelColors.Bg
            Icon(icon, contentDescription = null, tint = fg, modifier = Modifier.size(20.dp))
            Text(
                text,
                style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold),
                color = fg,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 8.dp),
            )
        }
    }
}

/** Square-ish secondary action: icon over a short label. */
@Composable
private fun TileButton(text: String, icon: ImageVector, onClick: () -> Unit, modifier: Modifier) {
    FocusCard(onClick = onClick, modifier = modifier, shape = RoundedCornerShape(8.dp)) { focused ->
        Column(
            Modifier.fillMaxWidth().background(if (focused) ReelColors.Ink else ReelColors.Bg2).height(56.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            val fg = if (focused) ReelColors.Bg else ReelColors.Ink
            Icon(icon, contentDescription = null, tint = fg, modifier = Modifier.size(20.dp))
            Text(text, style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold, fontSize = 12.sp), color = fg)
        }
    }
}

/** Season tabs: the selected one underlined in the accent. OK (or a tap) switches. */
@Composable
private fun SeasonTabs(seasons: List<ItemSummary>, selected: Long?, onSeason: (Long) -> Unit, memory: FocusMemory, edge: Dp) {
    Box(Modifier.padding(top = 8.dp)) {
        Box(Modifier.align(Alignment.BottomStart).padding(horizontal = edge).fillMaxWidth().height(1.dp).background(ReelColors.Ink.copy(alpha = 0.1f)))
        LazyRow(
            state = rememberLazyListState(),
            contentPadding = PaddingValues(horizontal = edge),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(seasons, key = { it.id }) { s ->
                val isSel = s.id == selected
                FocusCard(
                    onClick = { onSeason(s.id) },
                    shape = RoundedCornerShape(6.dp),
                    modifier = Modifier.focusMemory(memory, "season/${s.id}").testTag("season-${s.id}"),
                ) { focused ->
                    Column(Modifier.width(IntrinsicSize.Max).background(if (focused) ReelColors.Ink else Color.Transparent).padding(horizontal = 10.dp)) {
                        Text(
                            s.title,
                            style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold),
                            color = when {
                                focused -> ReelColors.Bg
                                isSel -> ReelColors.Ink
                                else -> ReelColors.Ink.copy(alpha = 0.6f)
                            },
                            modifier = Modifier.padding(vertical = 10.dp),
                        )
                        Box(Modifier.fillMaxWidth().height(3.dp).background(if (isSel) ReelColors.AccentHi else Color.Transparent))
                    }
                }
            }
        }
    }
}

/** Episodes as rows: a thumbnail with a progress bar, `3. Low Water`, `50 min · 18 min left`. */
private fun LazyListScope.episodeRows(episodes: LoadHandle<List<ItemSummary>>, memory: FocusMemory, edge: Dp, tv: Boolean) {
    when (val s = episodes.state) {
        Load.Loading -> item(key = "ep-loading") {
            Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = ReelColors.Accent) }
        }
        is Load.Failed -> item(key = "ep-error") { ErrorView(s.message, onRetry = episodes::retry, modifier = Modifier.height(160.dp)) }
        is Load.Ready -> if (s.value.isEmpty()) {
            item(key = "ep-empty") { EmptyView("No episodes yet.", Modifier.height(120.dp)) }
        } else {
            items(s.value, key = { "ep-${it.id}" }) { e -> EpisodeRow(e, memory, edge, tv) }
        }
    }
}

@Composable
private fun EpisodeRow(e: ItemSummary, memory: FocusMemory, edge: Dp, tv: Boolean) {
    val nav = LocalNavController.current
    FocusCard(
        onClick = { nav.navigate(Routes.item(e.id)) },
        shape = RoundedCornerShape(10.dp),
        modifier = Modifier.padding(horizontal = edge - 8.dp).fillMaxWidth().focusMemory(memory, "ep/${e.id}").testTag("ep-${e.id}"),
    ) { focused ->
        Row(
            Modifier.fillMaxWidth().background(if (focused) ReelColors.Bg3 else Color.Transparent).padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(if (tv) 200.dp else 120.dp).aspectRatio(16f / 9f).clip(RoundedCornerShape(8.dp))) {
                ReelImage(e.backdropUrl ?: e.posterUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
                val p = when {
                    e.played -> 1f
                    else -> e.progress?.toFloat()?.coerceIn(0f, 1f) ?: 0f
                }
                if (p > 0f) {
                    Box(Modifier.align(Alignment.BottomStart).fillMaxWidth().height(3.dp).background(ReelColors.Ink.copy(alpha = 0.16f)))
                    Box(Modifier.align(Alignment.BottomStart).fillMaxWidth(p).height(3.dp).background(ReelColors.AccentHi))
                }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    (e.episodeNo?.let { "$it. " } ?: "") + e.title,
                    style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold),
                    color = ReelColors.Ink,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                ItemLogic.episodeMeta(e)?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink.copy(alpha = 0.6f)) }
            }
        }
    }
}

/** Round photo + name + character. Focusable so D-pad can scroll the row on TV. */
@Composable
private fun CastCard(c: CastMember, modifier: Modifier) {
    Column(modifier.width(112.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        FocusCard(onClick = {}, shape = CircleShape, modifier = Modifier.size(88.dp)) {
            ReelImage(c.photoUrl, contentDescription = c.name, modifier = Modifier.fillMaxSize())
        }
        Text(c.name, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 8.dp))
        c.character?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3, maxLines = 1, overflow = TextOverflow.Ellipsis) }
    }
}

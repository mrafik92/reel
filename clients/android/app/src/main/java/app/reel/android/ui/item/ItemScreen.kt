package app.reel.android.ui.item

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.LocalBringIntoViewSpec
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.CastMember
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.children
import app.reel.android.data.item
import app.reel.android.data.setPlayed
import app.reel.android.ui.common.DetailActions
import app.reel.android.ui.common.DetailHeader
import app.reel.android.ui.common.WatchProgress
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
    val api = LocalAppGraph.current.session.api
    val scope = rememberCoroutineScope()
    val detail = rememberLoad(id) { item(id) }
    val memory = rememberFocusMemory()
    // Shows: which season's episodes are listed. Saved, so Back from an episode keeps it.
    var season by rememberSaveable(id) { mutableStateOf<Long?>(null) }
    Loadable(detail) { d ->
        val seasons = if (d.kind == "show") rememberLoad(d.id) { children(d.id) } else null
        val seasonList = (seasons?.state as? Load.Ready)?.value
        val selected = seasonList?.firstOrNull { it.id == season } ?: seasonList?.let(ItemLogic::defaultSeason)
        val episodesOf = if (d.kind == "season") d.id else selected?.id
        val episodes = episodesOf?.let { sid -> rememberLoad(sid) { children(sid) } }
        val next = (episodes?.state as? Load.Ready)?.value?.let(ItemLogic::nextUp)
        val episodeDetail = next?.let { rememberLoad(it.id) { item(it.id) } }
        val ready = seasons?.state !is Load.Loading && episodes?.state !is Load.Loading &&
            episodeDetail?.state !is Load.Loading
        var shown by remember { mutableStateOf(false) }
        LaunchedEffect(ready) { if (ready) shown = true }
        RestoreFocus(memory, ready = shown, first = "play")
        // Compose the lazy rows together on return so a saved episode scroll index can be restored.
        var played by remember(d.id, d.played) { mutableStateOf(d.played) }
        var saving by remember { mutableStateOf(false) }
        var error by remember { mutableStateOf<String?>(null) }
        if (shown || ready) {
            ItemBody(d, seasons, seasonList, selected?.id, { season = it }, episodes, memory) {
                Actions(d, next, episodeDetail, memory, played, saving, error) {
                    if (saving || api == null) return@Actions
                    val markPlayed = !played
                    saving = true
                    error = null
                    scope.launch {
                        try {
                            api.setPlayed(d.id, markPlayed)
                            played = markPlayed
                            episodes?.refresh()
                            episodeDetail?.refresh()
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            error = Load.Failed(e).message
                        } finally {
                            saving = false
                        }
                    }
                }
            }
        } else {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = ReelColors.Accent)
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ItemBody(
    d: ItemDetail,
    seasonLoad: LoadHandle<List<ItemSummary>>?,
    seasons: List<ItemSummary>?,
    season: Long?,
    onSeason: (Long) -> Unit,
    episodes: LoadHandle<List<ItemSummary>>?,
    memory: FocusMemory,
    actions: @Composable () -> Unit,
) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 48.dp else 20.dp
    CompositionLocalProvider(LocalBringIntoViewSpec provides KeepInView) {
        LazyColumn(
            Modifier.fillMaxSize().testTag("item"),
            contentPadding = PaddingValues(bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 14.dp),
        ) {
            item(key = "hero") {
                DetailHeader(
                    title = d.title,
                    kind = ItemLogic.kindLabel(d),
                    poster = d.posterUrl.takeUnless { d.kind == "episode" },
                    backdrop = d.backdropUrl ?: d.posterUrl,
                    facts = listOfNotNull(ItemLogic.stars(d.rating)) + ItemLogic.meta(d) + ItemLogic.badges(d.files.firstOrNull()),
                    actions = actions,
                )
            }
            d.overview?.takeIf { it.isNotBlank() }?.let { synopsis ->
                item(key = "synopsis") {
                    Text(synopsis, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2,
                        modifier = Modifier.padding(horizontal = edge).widthIn(max = 800.dp))
                }
            }

            if (!seasons.isNullOrEmpty() && seasons.size > 1) {
                item(key = "seasons") { SeasonTabs(seasons, season, onSeason, memory, edge) }
            } else if (d.kind == "season" || !seasons.isNullOrEmpty()) {
                item(key = "episodes-title") { SectionTitle("Episodes", edge) }
            }

            val seasonError = seasonLoad?.state as? Load.Failed
            if (seasonError != null) {
                item(key = "season-error") { ErrorView(seasonError.message, onRetry = seasonLoad::retry, modifier = Modifier.height(160.dp)) }
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

/** What the main button plays: the item itself, or a show's / season's next episode. */
private data class PlayTarget(val label: String, val play: () -> Unit, val restart: (() -> Unit)?)

/**
 * The main button (cream, full width on phones) names what it plays and the time left
 * (`Resume S2 · E3 · 18 min left`, `Resume from 38:13 · 52 min left`). Then *Play from start* (when
 * resuming) and *Mark watched*. Shows and seasons play their next episode.
 */
@Composable
private fun Actions(
    d: ItemDetail,
    next: ItemSummary?,
    nextDetail: LoadHandle<ItemDetail>?,
    memory: FocusMemory,
    played: Boolean,
    saving: Boolean,
    error: String?,
    togglePlayed: () -> Unit,
) {
    val nav = LocalNavController.current
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val playable = if (d.kind == "show" || d.kind == "season") (nextDetail?.state as? Load.Ready)?.value else d
    val target = playable?.let { item ->
        item.files.firstOrNull()?.let { file ->
            val progress = ItemLogic.resumeAt(item.progress, file.durationS, if (item.id == d.id) played else item.played)
            val left = if (progress > 0) ItemLogic.timeLeft(item.progress, file.durationS) else null
            val episode = ItemLogic.episodeCode(item.seasonNo, item.episodeNo) ?: next?.title
            PlayTarget(
                ItemLogic.playLabel(progress, episode, left),
                { nav.navigate(Routes.player(file.id, item.id, progress)) },
                if (progress > 0) ({ nav.navigate(Routes.player(file.id, item.id, 0.0)) }) else null,
            )
        }
    }

    val watchedLabel = if (played) "Mark unwatched" else "Mark watched"
    val watchedIcon = if (played) Icons.Outlined.VisibilityOff else Icons.Outlined.CheckCircle

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        DetailActions {
            target?.let { t ->
                Pill(
                    t.label,
                    icon = Icons.Outlined.PlayArrow,
                    primary = true,
                    onClick = t.play,
                    modifier = (if (tv) Modifier else Modifier.fillMaxWidth()).focusMemory(memory, "play").testTag("play"),
                )
                t.restart?.let { restart ->
                    Pill("Play from start", icon = Icons.Outlined.Replay, onClick = restart,
                        modifier = Modifier.focusMemory(memory, "restart").testTag("restart"))
                }
            }
            Pill(watchedLabel, icon = watchedIcon, onClick = togglePlayed,
                modifier = Modifier.focusMemory(memory, "played").testTag("played"))
            if (saving || nextDetail?.state is Load.Loading) CircularProgressIndicator(color = ReelColors.Accent, modifier = Modifier.size(28.dp))
        }
        if (playable?.files?.isEmpty() == true) {
            Text("No playable file for this title yet.", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        }
        (nextDetail?.state as? Load.Failed)?.let { failure ->
            ErrorView(failure.message, onRetry = nextDetail::retry)
        }
        error?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.AccentHi, modifier = Modifier.testTag("item-error")) }
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
                    modifier = Modifier.focusMemory(memory, "season/${s.id}").testTag("season-${s.id}").semantics { this.selected = isSel },
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
                            modifier = Modifier.heightIn(min = 45.dp).padding(vertical = 10.dp),
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
                WatchProgress(if (e.played) 1.0 else e.progress, Modifier.align(Alignment.BottomCenter).fillMaxWidth())
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

package app.reel.android.ui.live

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.reel.android.LocalAppGraph
import app.reel.android.data.IptvChannel
import app.reel.android.data.NowNext
import app.reel.android.data.channels
import app.reel.android.data.nowNext
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.OnResume
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.TopLevel
import app.reel.android.ui.shell.navigateTopLevel
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.focusMemory

/** What the home row shows: favourite channels with what's on now (at most [max]). */
internal data class LiveNowCard(val channel: IptvChannel, val number: String, val now: NowNext?)

internal fun liveNowCards(all: List<IptvChannel>, now: List<NowNext>, max: Int = 12): List<LiveNowCard> {
    val byId = now.associateBy { it.channelId }
    return all.filter { it.favorite }.take(max).map { LiveNowCard(it, LiveLogic.displayNumber(it, all), byId[it.id]) }
}

/**
 * Home's *Live now* row: the viewer's favourite channels and what's on. Picking one opens Live TV
 * on that channel. Hidden when there are no favourites (or Live TV fails to load).
 */
@Composable
internal fun LiveNowRow(memory: FocusMemory, edge: Dp, tv: Boolean, emptyText: String? = null) {
    val load = rememberLoad {
        val all = channels()
        val favs = all.filter { it.favorite }
        val now = if (favs.isEmpty()) emptyList() else runCatching { nowNext(favs.take(12).map { it.id }) }.getOrDefault(emptyList())
        liveNowCards(all, now)
    }
    OnResume { load.refresh() }
    val cards = (load.state as? Load.Ready)?.value.orEmpty()
    if (cards.isEmpty()) {
        if (emptyText != null && load.state !is Load.Loading) EmptyView(emptyText)
        return
    }
    val graph = LocalAppGraph.current
    val nav = LocalNavController.current
    val nowS = System.currentTimeMillis() / 1000
    Column(Modifier.testTag("live-now")) {
        Text(
            "Live now",
            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.ExtraBold),
            color = ReelColors.Ink,
            modifier = Modifier.padding(start = edge, bottom = 10.dp),
        )
        LazyRow(
            state = rememberLazyListState(),
            contentPadding = PaddingValues(horizontal = edge, vertical = if (tv) 12.dp else 4.dp),
            horizontalArrangement = Arrangement.spacedBy(if (tv) 20.dp else 10.dp),
        ) {
            items(cards, key = { it.channel.id }) { card ->
                val (now, _) = LiveLogic.current(card.now, nowS)
                FocusCard(
                    onClick = {
                        graph.liveRequest.value = card.channel.id
                        nav.navigateTopLevel(TopLevel.Live)
                    },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.width(if (tv) 300.dp else 240.dp).focusMemory(memory, "live/${card.channel.id}").testTag("live-now-${card.channel.id}"),
                ) { focused ->
                    Row(
                        Modifier.fillMaxWidth().background(if (focused) ReelColors.Bg3 else ReelColors.Bg2).padding(10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        ChannelTile(card.channel, card.number, 52.dp)
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text("${card.number} · ${card.channel.name}", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(now?.title ?: card.channel.name, style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (now != null) ProgrammeBar(LiveLogic.progress(now, nowS), Modifier.fillMaxWidth())
                        }
                    }
                }
            }
        }
    }
}

package app.reel.android.ui.live

import android.view.KeyEvent
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.reel.android.LocalAppGraph
import app.reel.android.data.IptvChannel
import app.reel.android.data.Program
import app.reel.android.data.schedule
import app.reel.android.player.LivePhase
import app.reel.android.player.ReelPlayer
import app.reel.android.player.Tabular
import app.reel.android.ui.common.Pill
import app.reel.android.ui.shell.LocalShellChrome
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.delay

/**
 * TV Live TV: the channel list (OK tunes and goes full screen, hold OK for favourites), and the
 * full-screen player: a channel banner (number, LIVE, now with start → end, the next two), and a
 * side channel list that fades with distance from the current channel. D-pad up/down (and CH+/−)
 * zap, digits type a channel number, left/right/OK open the side list, Back closes it, then
 * returns to the list and frees the provider slot.
 */
@Composable
internal fun TvLive(s: LiveScreenState, player: ReelPlayer, openFull: Long?) {
    var full by remember { mutableStateOf(false) }
    val memory = rememberFocusMemory()
    val listState = rememberLazyListState()
    LaunchedEffect(openFull) { if (openFull != null) full = true }
    val chrome = LocalShellChrome.current
    DisposableEffect(full) {
        chrome.hidden = full
        onDispose { chrome.hidden = false }
    }
    if (full) {
        TvFull(s, player, exit = {
            s.controller.stop()
            full = false
        })
    } else {
        TvBrowse(s, memory, listState, open = { c ->
            s.controller.tune(c.id)
            full = true
        })
    }
}

@Composable
private fun TvBrowse(s: LiveScreenState, memory: FocusMemory, listState: LazyListState, open: (IptvChannel) -> Unit) {
    RestoreFocus(memory, first = s.visible.firstOrNull()?.let { "ch/${it.id}" } ?: "chip/All")
    Column(Modifier.fillMaxSize().padding(top = 28.dp).testTag("live")) {
        Row(Modifier.padding(horizontal = 32.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("Live TV", style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
            Text("OK to watch · hold OK to add to favourites", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        }
        LazyRow(
            contentPadding = PaddingValues(horizontal = 32.dp, vertical = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.testTag("live-chips"),
        ) {
            items(s.ui.chips, key = { it.label + (it as? ChannelFilter.Group)?.let { "-g" }.orEmpty() }) { f ->
                Pill(f.label, onClick = { s.vm.select(f) }, selected = f == s.ui.filter, modifier = Modifier.focusMemory(memory, "chip/${f.label}").testTag("chip-${f.label}"))
            }
        }
        val list = s.visible
        if (list.isEmpty()) {
            Text(
                if (s.ui.filter == ChannelFilter.Favourites) "No favourites yet. Hold OK on a channel to add it here." else "No channels in this group.",
                color = ReelColors.Ink2,
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.padding(32.dp),
            )
            return@Column
        }
        WatchVisibleChannels(listState, s.updateVisible)
        LazyColumn(
            state = listState,
            contentPadding = PaddingValues(start = 32.dp, end = 48.dp, top = 4.dp, bottom = 48.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxSize().testTag("live-channels"),
        ) {
            items(list, key = { it.id }) { c ->
                val (now, next) = s.nowNext(c)
                FocusCard(
                    onClick = { open(c) },
                    onLongClick = { s.vm.toggleFavorite(c.id) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth().focusMemory(memory, "ch/${c.id}").testTag("channel-${c.id}"),
                ) { focused ->
                    Row(
                        Modifier.fillMaxWidth().background(if (focused) ReelColors.Bg3 else ReelColors.Bg1).padding(horizontal = 18.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(18.dp),
                    ) {
                        Text(s.number(c), fontSize = 22.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, style = Tabular, modifier = Modifier.width(64.dp))
                        ChannelTile(c, s.number(c), 56.dp)
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(c.name, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2, maxLines = 1)
                            Text(now?.title ?: "No guide for this channel", style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (now != null) ProgrammeBar(LiveLogic.progress(now, s.ui.nowS), Modifier.width(160.dp), height = 4.dp)
                        }
                        next?.let {
                            Text(
                                "Next ${LiveLogic.clock(it.start)} ${it.title}",
                                style = MaterialTheme.typography.bodyMedium,
                                color = ReelColors.Ink3,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.width(320.dp),
                            )
                        }
                        if (c.favorite) Icon(Icons.Filled.Star, contentDescription = "Favourite", tint = StarGold)
                    }
                }
            }
        }
    }
}

private val DIGIT_KEYS = (0..9).associateBy { KeyEvent.KEYCODE_0 + it } + (0..9).associateBy { KeyEvent.KEYCODE_NUMPAD_0 + it }
private val OK_KEYS = setOf(KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER)

@Composable
private fun TvFull(s: LiveScreenState, player: ReelPlayer, exit: () -> Unit) {
    val api = LocalAppGraph.current.session.api
    val root = remember { FocusRequester() }
    val entry = remember { NumberEntry() }
    var typed by remember { mutableStateOf("") }
    var typedAt by remember { mutableIntStateOf(0) }
    var notice by remember { mutableStateOf<String?>(null) }
    var strip by remember { mutableStateOf(false) }
    var cursor by remember { mutableStateOf<Long?>(null) }
    var poke by remember { mutableIntStateOf(0) }
    var banner by remember { mutableStateOf(true) }
    val view = LocalView.current
    DisposableEffect(Unit) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }

    LaunchedEffect(s.live.error) { if (s.live.error == null) runCatching { root.requestFocus() } }
    // The banner shows for a while after each zap / key, and while the side list is open.
    LaunchedEffect(poke, strip, s.live.phase) {
        banner = true
        if (!strip && s.live.phase == LivePhase.Playing) {
            delay(5_000)
            banner = false
        }
    }
    val sideChannels = if (strip) channelsAround(s.visible, cursor) else emptyList()
    LaunchedEffect(sideChannels) { s.updateVisible(sideChannels.map { it.id }) }
    LaunchedEffect(poke, strip) {
        if (strip) {
            delay(8_000)
            strip = false
        }
    }
    // Digits: tune once the viewer stops typing (or the number is complete).
    val commitNumber: () -> Unit = {
        val n = entry.commit()
        typed = ""
        if (n != null) {
            val c = LiveLogic.byNumber(s.visible, s.all, n)
            if (c != null) s.controller.tune(c.id) else notice = "No channel $n"
            poke++
        }
    }
    LaunchedEffect(typedAt) {
        if (typed.isEmpty()) return@LaunchedEffect
        delay(NumberEntry.TIMEOUT_MS)
        commitNumber()
    }
    LaunchedEffect(notice) {
        if (notice != null) {
            delay(2_000)
            notice = null
        }
    }
    // The next two programmes (the guide window of the channel on screen).
    val current = s.current
    var later by remember(current?.id) { mutableStateOf<List<Program>>(emptyList()) }
    LaunchedEffect(current?.id, s.nowNext(current).first?.start) {
        val c = current ?: return@LaunchedEffect
        if (api == null || !c.hasEpg) return@LaunchedEffect
        delay(800) // once zapping settles
        val t = System.currentTimeMillis() / 1000
        later = runCatching { api.schedule(c.id, t, t + 6 * 3600) }.getOrDefault(emptyList())
    }

    val back: () -> Unit = {
        when {
            strip -> strip = false
            typed.isNotEmpty() -> {
                entry.clear()
                typed = ""
            }
            else -> exit()
        }
    }
    BackHandler(onBack = back)

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onPreviewKeyEvent { e ->
                val code = e.key.nativeKeyCode
                if (code == KeyEvent.KEYCODE_BACK) {
                    if (e.type == KeyEventType.KeyUp) back()
                    return@onPreviewKeyEvent true
                }
                if (e.type != KeyEventType.KeyDown || s.live.error != null) return@onPreviewKeyEvent false
                DIGIT_KEYS[code]?.let { d ->
                    val done = entry.type(d)
                    typed = entry.digits
                    if (done) commitNumber() else typedAt++
                    return@onPreviewKeyEvent true
                }
                poke++
                if (strip) {
                    val list = s.visible
                    val at = list.indexOfFirst { it.id == cursor }.coerceAtLeast(0)
                    when (code) {
                        KeyEvent.KEYCODE_DPAD_UP -> cursor = list.getOrNull((at - 1).coerceAtLeast(0))?.id
                        KeyEvent.KEYCODE_DPAD_DOWN -> cursor = list.getOrNull((at + 1).coerceAtMost(list.lastIndex))?.id
                        in OK_KEYS -> cursor?.let { if (it != s.live.channelId) s.controller.tune(it) }
                        KeyEvent.KEYCODE_DPAD_LEFT -> strip = false
                        KeyEvent.KEYCODE_CHANNEL_UP -> s.zap(1)
                        KeyEvent.KEYCODE_CHANNEL_DOWN -> s.zap(-1)
                        else -> return@onPreviewKeyEvent false
                    }
                    return@onPreviewKeyEvent true
                }
                when (code) {
                    // Channel up = the next channel in the list, like a TV remote.
                    KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_CHANNEL_UP -> s.zap(1)
                    KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_CHANNEL_DOWN -> s.zap(-1)
                    KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_RIGHT, in OK_KEYS -> {
                        strip = true
                        cursor = s.live.channelId ?: s.current?.id
                    }
                    KeyEvent.KEYCODE_INFO, KeyEvent.KEYCODE_GUIDE -> Unit // shows the banner (poke)
                    KeyEvent.KEYCODE_MEDIA_STOP -> exit()
                    else -> return@onPreviewKeyEvent false
                }
                true
            }
            .focusRequester(root)
            .focusable()
            .testTag("live-full"),
    ) {
        LiveVideo(player, Modifier.fillMaxSize())
        val live = s.live
        if (live.phase == LivePhase.Tuning && live.error == null) {
            CircularProgressIndicator(Modifier.align(Alignment.Center).testTag("live-buffering"), color = LiveRose)
        }
        live.recovering?.let { Text(it, color = ReelColors.Ink, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.align(Alignment.Center).padding(top = 110.dp)) }

        AnimatedVisibility(visible = banner && live.error == null, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxSize()) {
                Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().fillMaxHeight(0.45f).background(Brush.verticalGradient(listOf(Color.Transparent, ReelColors.Bg.copy(alpha = 0.95f)))))
                Row(Modifier.padding(start = 64.dp, top = 40.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    for (h in listOf("▲▼ Zap", "◀ ▶ Channel list", "0–9 Number")) {
                        Text(
                            h,
                            style = MaterialTheme.typography.bodyMedium,
                            color = ReelColors.Ink.copy(alpha = 0.7f),
                            modifier = Modifier.background(ReelColors.Bg.copy(alpha = 0.55f), RoundedCornerShape(16.dp)).padding(horizontal = 12.dp, vertical = 6.dp),
                        )
                    }
                }
                TvBanner(s, later, Modifier.align(Alignment.BottomStart).padding(start = 64.dp, end = if (strip) 380.dp else 64.dp, bottom = 48.dp))
            }
        }
        AnimatedVisibility(visible = strip, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.align(Alignment.CenterEnd)) {
            SideStrip(s, cursor ?: s.current?.id)
        }
        if (typed.isNotEmpty() || notice != null) {
            Text(
                notice ?: typed,
                fontSize = if (notice != null) 28.sp else 64.sp,
                fontWeight = FontWeight.ExtraBold,
                color = ReelColors.Ink,
                style = Tabular,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(40.dp)
                    .background(ReelColors.Bg.copy(alpha = 0.7f), RoundedCornerShape(12.dp))
                    .padding(horizontal = 20.dp, vertical = 8.dp)
                    .testTag("live-number"),
            )
        }
        live.error?.let { err ->
            LiveErrorPanel(err, retry = s.controller::retry, modifier = Modifier.align(Alignment.Center), onBack = exit)
        }
    }
}

/** Number + LIVE; name · group; now with start → end; the next two. */
@Composable
private fun TvBanner(s: LiveScreenState, later: List<Program>?, modifier: Modifier) {
    val c = s.current ?: return
    val (now, next) = s.nowNext(c)
    val upcoming = later?.filter { p -> p.start >= (now?.stop ?: s.ui.nowS) }?.take(2)?.takeIf { it.isNotEmpty() } ?: listOfNotNull(next)
    Row(modifier.testTag("live-banner"), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(32.dp)) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(s.number(c), fontSize = 72.sp, lineHeight = 72.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, style = Tabular)
            LiveBadge(big = true)
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(listOfNotNull(c.name, c.group).joinToString(" · "), fontSize = 16.sp, color = ReelColors.Ink2, maxLines = 1)
            Text(now?.title ?: c.name, fontSize = 40.sp, lineHeight = 42.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (now != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    Text(LiveLogic.clock(now.start), fontSize = 15.sp, color = ReelColors.Ink, style = Tabular)
                    ProgrammeBar(LiveLogic.progress(now, s.ui.nowS), Modifier.weight(1f), height = 5.dp)
                    Text(LiveLogic.clock(now.stop), fontSize = 15.sp, color = ReelColors.Ink, style = Tabular)
                }
            }
            if (upcoming.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                    upcoming.forEachIndexed { i, p ->
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (i == 0) Text("Next", fontSize = 16.sp, fontWeight = FontWeight.Bold, color = ReelColors.Ink)
                            Text("${LiveLogic.clock(p.start)} ${p.title}", fontSize = 16.sp, color = ReelColors.Ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        }
    }
}

private fun channelsAround(list: List<IptvChannel>, cursorId: Long?): List<IptvChannel> {
    val at = list.indexOfFirst { it.id == cursorId }.coerceAtLeast(0)
    return list.subList((at - 3).coerceAtLeast(0), (at + 4).coerceAtMost(list.size))
}

/** Seven channels around [cursorId], fading with distance; the cursor one is cream. */
@Composable
private fun SideStrip(s: LiveScreenState, cursorId: Long?) {
    val list = s.visible
    val at = list.indexOfFirst { it.id == cursorId }.coerceAtLeast(0)
    Column(
        Modifier
            .fillMaxHeight()
            .width(340.dp)
            .background(Brush.horizontalGradient(0f to Color.Transparent, 0.4f to ReelColors.Bg.copy(alpha = 0.92f)))
            .padding(horizontal = 24.dp)
            .testTag("live-strip"),
        verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically),
    ) {
        for (i in (at - 3)..(at + 3)) {
            val c = list.getOrNull(i)
            if (c == null) {
                Box(Modifier.height(64.dp))
                continue
            }
            val cur = i == at
            val (now, _) = s.nowNext(c)
            Row(
                Modifier
                    .fillMaxWidth()
                    .height(64.dp)
                    .alpha(1f - kotlin.math.abs(i - at) * 0.18f)
                    .background(if (cur) ReelColors.Ink else Color.Transparent, RoundedCornerShape(10.dp))
                    .padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                val fg = if (cur) ReelColors.Bg else ReelColors.Ink
                Text(s.number(c), fontSize = 18.sp, fontWeight = FontWeight.ExtraBold, color = fg, style = Tabular, modifier = Modifier.width(48.dp), maxLines = 1)
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(c.name, fontSize = 13.sp, color = fg.copy(alpha = 0.7f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(now?.title ?: "—", fontSize = 16.sp, fontWeight = FontWeight.Bold, color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

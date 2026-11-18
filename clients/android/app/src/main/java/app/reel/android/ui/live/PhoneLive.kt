package app.reel.android.ui.live

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Fullscreen
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.reel.android.data.IptvChannel
import app.reel.android.player.CtrlButton
import app.reel.android.player.ImmersiveLandscape
import app.reel.android.player.LivePhase
import app.reel.android.player.ReelPlayer
import app.reel.android.player.TopShade
import app.reel.android.ui.shell.LocalShellChrome
import app.reel.android.ui.theme.ReelColors
import kotlinx.coroutines.delay

/**
 * Phone Live TV: a docked 16:9 player on top (LIVE, "101 · Channel", the programme, full screen),
 * group chips with Favourites first, and the channel list. Tapping a channel tunes it; tapping the
 * one playing goes full screen. Full screen is landscape: swipe up/down or the arrows to zap.
 */
@Composable
internal fun PhoneLive(s: LiveScreenState, player: ReelPlayer) {
    var full by remember { mutableStateOf(false) }
    val chrome = LocalShellChrome.current
    DisposableEffect(full) {
        chrome.hidden = full
        onDispose { chrome.hidden = false }
    }
    if (full) ImmersiveLandscape()
    BackHandler(enabled = full) { full = false }
    val view = LocalView.current
    val watching = s.live.phase != LivePhase.Idle
    DisposableEffect(watching) {
        view.keepScreenOn = watching
        onDispose { view.keepScreenOn = false }
    }

    Column(Modifier.fillMaxSize().background(if (full) Color.Black else ReelColors.Bg).testTag("live")) {
        // The same node docked and full screen, so the video surface isn't rebuilt.
        Box(
            (if (full) Modifier.fillMaxSize() else Modifier.fillMaxWidth().aspectRatio(16f / 9f))
                .background(Color.Black)
                .testTag("live-player"),
        ) {
            LiveVideo(player, Modifier.fillMaxSize())
            if (full) FullOverlay(s, onExit = { full = false }) else DockedOverlay(s, onFull = { full = true })
        }
        if (!full) {
            Chips(s)
            ChannelList(s, onPlaying = { full = true })
        }
    }
}

@Composable
private fun DockedOverlay(s: LiveScreenState, onFull: () -> Unit) {
    val c = s.current
    val (now, _) = s.nowNext(c)
    val live = s.live
    Box(Modifier.fillMaxSize()) {
        // Before anything plays (and after Stop), a still card with an explicit Watch: tuning takes
        // the provider's only connection, so it never happens just by opening the tab.
        if (live.phase == LivePhase.Idle) {
            Box(Modifier.fillMaxSize().background(Brush.linearGradient(listOf(Color(0xFF24404F), Color(0xFF1D2A33), Color(0xFF101418)))))
            if (c != null) {
                Box(
                    Modifier
                        .align(Alignment.Center)
                        .size(64.dp)
                        .background(Color(0x80000000), CircleShape)
                        .clickable { s.controller.tune(c.id) }
                        .semantics { contentDescription = "Watch ${c.name}" }
                        .testTag("live-watch"),
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Filled.PlayArrow, contentDescription = null, tint = ReelColors.Ink, modifier = Modifier.size(36.dp)) }
            }
        }
        Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(90.dp).background(Brush.verticalGradient(listOf(Color.Transparent, ReelColors.Bg.copy(alpha = 0.85f)))))
        if (live.phase != LivePhase.Idle) LiveBadge(Modifier.align(Alignment.TopStart).padding(12.dp))
        if (live.phase == LivePhase.Tuning && live.error == null) {
            CircularProgressIndicator(Modifier.align(Alignment.Center).testTag("live-buffering"), color = LiveRose)
        }
        live.recovering?.let { Text(it, color = ReelColors.Ink, modifier = Modifier.align(Alignment.Center).padding(top = 72.dp)) }
        if (c != null) {
            Row(
                Modifier.align(Alignment.BottomStart).fillMaxWidth().padding(start = 14.dp, end = 8.dp, bottom = 10.dp),
                verticalAlignment = Alignment.Bottom,
            ) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    Text("${s.number(c)} · ${c.name}", fontSize = 12.sp, color = ReelColors.Ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        now?.title ?: c.name,
                        fontSize = 18.sp,
                        fontWeight = FontWeight.ExtraBold,
                        color = ReelColors.Ink,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.testTag("live-now-title"),
                    )
                }
                if (live.phase != LivePhase.Idle) {
                    RoundIcon(Icons.Filled.Stop, "Stop watching", { s.controller.stop() }, Modifier.testTag("live-stop"))
                    Spacer(Modifier.width(6.dp))
                    RoundIcon(Icons.Filled.Fullscreen, "Full screen", onFull, Modifier.testTag("live-fullscreen"))
                }
            }
        }
        live.error?.let { err ->
            LiveErrorPanel(err, retry = s.controller::retry, modifier = Modifier.align(Alignment.Center).padding(8.dp), compact = true)
        }
    }
}

@Composable
private fun RoundIcon(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, onClick: () -> Unit, modifier: Modifier = Modifier) {
    IconButton(onClick = onClick, modifier = modifier.size(44.dp).background(ReelColors.Bg.copy(alpha = 0.5f), CircleShape)) {
        Icon(icon, contentDescription = label, tint = ReelColors.Ink, modifier = Modifier.size(22.dp))
    }
}

@Composable
private fun Chips(s: LiveScreenState) {
    LazyRow(
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.testTag("live-chips"),
    ) {
        items(s.ui.chips, key = { it.label + (it as? ChannelFilter.Group)?.let { "-g" }.orEmpty() }) { f ->
            val on = f == s.ui.filter
            Box(
                Modifier
                    .height(36.dp)
                    .border(1.dp, if (on) ReelColors.Ink else ReelColors.Ink.copy(alpha = 0.24f), RoundedCornerShape(18.dp))
                    .background(if (on) ReelColors.Ink else Color.Transparent, RoundedCornerShape(18.dp))
                    .clickable { s.vm.select(f) }
                    .semantics { selected = on }
                    .padding(horizontal = 14.dp)
                    .testTag("chip-${f.label}"),
                contentAlignment = Alignment.Center,
            ) {
                Text(f.label, fontSize = 13.sp, fontWeight = FontWeight.Bold, color = if (on) ReelColors.Bg else ReelColors.Ink, maxLines = 1)
            }
        }
    }
}

@Composable
private fun ChannelList(s: LiveScreenState, onPlaying: () -> Unit) {
    val list = s.visible
    if (list.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.TopCenter) {
            Text(
                if (s.ui.filter == ChannelFilter.Favourites) "No favourites yet. Tap the star on a channel to add it here." else "No channels in this group.",
                color = ReelColors.Ink2,
                style = MaterialTheme.typography.bodyLarge,
            )
        }
        return
    }
    val state = rememberLazyListState()
    WatchVisibleChannels(state, s.updateVisible)
    LazyColumn(
        state = state,
        contentPadding = PaddingValues(start = 8.dp, end = 8.dp, bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
        modifier = Modifier.fillMaxSize().testTag("live-channels"),
    ) {
        items(list, key = { it.id }) { c ->
            val playing = c.id == s.live.channelId && s.live.phase != LivePhase.Idle
            ChannelRow(
                s,
                c,
                playing,
                onClick = { if (playing && s.live.phase == LivePhase.Playing) onPlaying() else s.controller.tune(c.id) },
            )
        }
    }
}

@Composable
private fun ChannelRow(s: LiveScreenState, c: IptvChannel, playing: Boolean, onClick: () -> Unit) {
    val (now, next) = s.nowNext(c)
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (playing) LiveRose.copy(alpha = 0.1f) else Color.Transparent, RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .semantics { contentDescription = "${s.number(c)} · ${c.name}" }
            .padding(horizontal = 8.dp, vertical = 10.dp)
            .testTag("channel-${c.id}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        ChannelTile(c, s.number(c), 52.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                now?.title ?: c.name,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                color = if (playing) LiveRose else ReelColors.Ink,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (now != null) ProgrammeBar(LiveLogic.progress(now, s.ui.nowS), Modifier.width(72.dp))
                Text(
                    next?.let { "Next ${LiveLogic.clock(it.start)} ${it.title}" } ?: if (now != null) c.name else "${s.number(c)} · ${c.group ?: "No guide"}",
                    fontSize = 12.sp,
                    color = ReelColors.Ink.copy(alpha = 0.6f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        IconButton(onClick = { s.vm.toggleFavorite(c.id) }, modifier = Modifier.size(44.dp).testTag("fav-${c.id}")) {
            Icon(
                if (c.favorite) Icons.Filled.Star else Icons.Filled.StarBorder,
                contentDescription = if (c.favorite) "Remove from favourites" else "Add to favourites",
                tint = if (c.favorite) StarGold else ReelColors.Ink.copy(alpha = 0.5f),
            )
        }
    }
}

/**
 * Full screen (landscape): tap shows the controls; swipe up = next channel, down = previous; the
 * arrows do the same. A zap shows the channel banner for a few seconds.
 */
@Composable
private fun FullOverlay(s: LiveScreenState, onExit: () -> Unit) {
    var visible by remember { mutableStateOf(true) }
    var poke by remember { mutableIntStateOf(0) }
    LaunchedEffect(visible, poke, s.live.phase) {
        if (visible && s.live.phase == LivePhase.Playing) {
            delay(4_000)
            visible = false
        }
    }
    val zap: (Int) -> Unit = { d ->
        s.zap(d)
        visible = true
        poke++
    }
    val swipeZap by rememberUpdatedState(zap)
    val threshold = with(LocalDensity.current) { 64.dp.toPx() }
    var drag by remember { mutableFloatStateOf(0f) }
    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(Unit) {
                detectTapGestures(onTap = {
                    visible = !visible
                    poke++
                })
            }
            .pointerInput(threshold) {
                detectVerticalDragGestures(
                    onDragStart = { drag = 0f },
                    onDragEnd = {
                        // Swipe up (like scrolling a list down) = the next channel.
                        if (drag < -threshold) swipeZap(1) else if (drag > threshold) swipeZap(-1)
                        drag = 0f
                    },
                    onVerticalDrag = { _, dy -> drag += dy },
                )
            }
            .testTag("live-full"),
    ) {
        val live = s.live
        if (live.phase == LivePhase.Tuning && live.error == null) {
            CircularProgressIndicator(Modifier.align(Alignment.Center), color = LiveRose)
        }
        live.recovering?.let { Text(it, color = ReelColors.Ink, modifier = Modifier.align(Alignment.Center).padding(top = 96.dp)) }
        AnimatedVisibility(visible = visible && live.error == null, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxSize()) {
                Row(
                    Modifier.fillMaxWidth().background(TopShade).safeDrawingPadding().padding(horizontal = 24.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    CtrlButton(Icons.AutoMirrored.Filled.ArrowBack, "Exit full screen", onExit)
                    LiveBadge()
                }
                Column(
                    Modifier.align(Alignment.CenterEnd).safeDrawingPadding().padding(end = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(24.dp),
                ) {
                    CtrlButton(Icons.Filled.KeyboardArrowUp, "Next channel", { zap(1) }, Modifier.testTag("zap-next"))
                    CtrlButton(Icons.Filled.KeyboardArrowDown, "Previous channel", { zap(-1) }, Modifier.testTag("zap-prev"))
                }
                ChannelBanner(s, Modifier.align(Alignment.BottomStart).fillMaxWidth())
            }
        }
        live.error?.let { err ->
            LiveErrorPanel(err, retry = s.controller::retry, modifier = Modifier.align(Alignment.Center), onBack = onExit)
        }
    }
}

/** Number, name, LIVE, now with its progress from start to end, and next. */
@Composable
private fun ChannelBanner(s: LiveScreenState, modifier: Modifier) {
    val c = s.current ?: return
    val (now, next) = s.nowNext(c)
    Row(
        modifier
            .background(Brush.verticalGradient(listOf(Color.Transparent, ReelColors.Bg.copy(alpha = 0.92f))))
            .safeDrawingPadding()
            .padding(horizontal = 32.dp, vertical = 20.dp)
            .testTag("live-banner"),
        verticalAlignment = Alignment.Bottom,
        horizontalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        Text(s.number(c), fontSize = 44.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, style = app.reel.android.player.Tabular)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(listOfNotNull(c.name, c.group).joinToString(" · "), fontSize = 14.sp, color = ReelColors.Ink2, maxLines = 1)
            Text(now?.title ?: c.name, fontSize = 24.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (now != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(LiveLogic.clock(now.start), fontSize = 13.sp, color = ReelColors.Ink)
                    ProgrammeBar(LiveLogic.progress(now, s.ui.nowS), Modifier.weight(1f), height = 4.dp)
                    Text(LiveLogic.clock(now.stop), fontSize = 13.sp, color = ReelColors.Ink)
                }
            }
            next?.let { Text("Next  ${LiveLogic.clock(it.start)} ${it.title}", fontSize = 14.sp, color = ReelColors.Ink2, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
    }
}

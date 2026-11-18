package app.reel.android.player

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ActivityInfo
import android.os.SystemClock
import android.view.ViewGroup
import androidx.activity.compose.BackHandler
import androidx.annotation.OptIn
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Forward10
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Replay10
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.media3.common.util.UnstableApi
import androidx.media3.ui.AspectRatioFrameLayout
import androidx.media3.ui.PlayerView
import app.reel.android.BuildConfig
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.ReelApi
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

private enum class Menu { Audio, Subtitles, Quality, Info }

/** Seconds of the *Up next* countdown at the end of an episode. */
private const val UP_NEXT_S = 10

/**
 * The VOD player screen (route `player/{fileId}`, see [Routes.player]): full screen, landscape and
 * immersive on phones, D-pad and media keys on TV. Leaving it ends the server session.
 */
@Composable
fun PlayerScreen(fileId: Long, itemId: Long, startS: Double) {
    val graph = LocalAppGraph.current
    val api = graph.session.api ?: return
    // The item gives the title, the duration (5-s rule) and the season (next episode).
    var detail by remember(itemId) { mutableStateOf<Result<ItemDetail>?>(null) }
    LaunchedEffect(itemId) { detail = runCatching { api.get<ItemDetail>("/items/$itemId") } }
    var deviceId by remember { mutableStateOf<String?>(null) }
    var quality by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        // This device's starting quality (Settings, #194); the quality menu overrides it per play.
        quality = runCatching { graph.devicePrefs.quality.first() }.getOrNull() ?: "auto"
        deviceId = runCatching { graph.session.deviceId() }.getOrNull() ?: ""
    }
    val d = detail
    val dev = deviceId
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        val q = quality
        if (d == null || dev == null || q == null) {
            CircularProgressIndicator(Modifier.align(Alignment.Center), color = ReelColors.Accent)
        } else {
            val item = d.getOrNull()
            val source = remember(fileId, itemId) { LibrarySource(api, fileId, itemId, dev.ifEmpty { null }) }
            VodPlayer(
                source = source,
                api = api,
                item = item,
                durationS = item?.files?.firstOrNull { it.id == fileId }?.durationS,
                startS = startS,
                initialQuality = q,
            )
        }
    }
}

/**
 * The player UI on a [PlaySource]: library files now, IPTV VOD (#193) later. [item] may be a
 * stand-in (title, kind, season / episode numbers); next-episode only works for library items.
 */
@OptIn(UnstableApi::class)
@Composable
fun VodPlayer(
    source: PlaySource,
    api: ReelApi,
    item: ItemDetail?,
    durationS: Double?,
    startS: Double,
    nextEnabled: Boolean = true,
    /** The top line (a show's title) when [item] is a stand-in without a library parent. */
    heading: String? = null,
    /** The device's default quality (`AppGraph.devicePrefs`). */
    initialQuality: String = "auto",
) {
    val graph = LocalAppGraph.current
    val context = LocalContext.current
    val nav = LocalNavController.current
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val scope = rememberCoroutineScope()

    val player = remember { ReelPlayer(context, graph.http, "Reel-Android/${BuildConfig.VERSION_NAME}") }
    val controller = remember(source) {
        VodController(
            source = source,
            player = player,
            scope = graph.scope,
            resolve = { url -> api.server.url.resolve(url)?.toString() ?: url },
            now = SystemClock::elapsedRealtime,
            knownDurationS = durationS,
            initialQuality = initialQuality,
        )
    }
    DisposableEffect(controller) {
        controller.start(startS)
        onDispose {
            controller.close()
            player.release()
        }
    }
    val st by controller.state.collectAsStateWithLifecycle()

    // Pause when the app goes to the background (no background audio in v1).
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e -> if (e == Lifecycle.Event.ON_STOP) controller.pause() }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }
    if (!tv) ImmersiveLandscape()
    val view = LocalView.current
    DisposableEffect(Unit) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }

    // ---- next episode ----
    var nextEp by remember { mutableStateOf<ItemSummary?>(null) }
    var series by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(item?.id) {
        val parent = item?.parentId?.takeIf { item.kind == "episode" } ?: return@LaunchedEffect
        if (nextEnabled) nextEp = runCatching { nextEpisode(item, api.get<List<ItemSummary>>("/items/$parent/children")) }.getOrNull()
        // Episode → season → show, for the title bar.
        series = runCatching {
            val s = api.get<ItemDetail>("/items/$parent")
            s.parentId?.let { api.get<ItemDetail>("/items/$it").title } ?: s.title
        }.getOrNull()
    }
    val playNext: () -> Unit = {
        nextEp?.let { n ->
            scope.launch {
                val nd = runCatching { api.get<ItemDetail>("/items/${n.id}") }.getOrNull() ?: return@launch
                val f = nd.files.firstOrNull() ?: return@launch
                nav.navigate(Routes.player(f.id, nd.id, resumePositionS(nd))) {
                    popUpTo(Routes.PLAYER) { inclusive = true }
                }
            }
        }
    }
    val leave: () -> Unit = { nav.popBackStack() }

    // ---- controls ----
    var visible by remember { mutableStateOf(true) }
    var poke by remember { mutableIntStateOf(0) }
    var menu by remember { mutableStateOf<Menu?>(null) }
    var pos by remember { mutableLongStateOf((startS * 1000).toLong()) }
    var dur by remember { mutableStateOf<Long?>(durationS?.let { (it * 1000).toLong() }) }
    var buffered by remember { mutableLongStateOf(0) }
    val rootFocus = remember { FocusRequester() }
    val playFocus = remember { FocusRequester() }
    val seekFocus = remember { FocusRequester() }
    var focusSeek by remember { mutableStateOf(false) }

    LaunchedEffect(player) {
        while (true) {
            pos = player.positionMs
            player.durationMs?.let { dur = it }
            buffered = player.exo.bufferedPosition
            delay(250)
        }
    }
    // Auto-hide: 3 s on TV, 4 s on phones, only while playing and no menu is open.
    LaunchedEffect(visible, poke, st.playing, menu) {
        if (visible && st.playing && menu == null) {
            delay(if (tv) 3_000 else 4_000)
            visible = false
        }
    }
    LaunchedEffect(visible) {
        if (!tv) return@LaunchedEffect
        if (visible) {
            delay(50)
            runCatching { if (focusSeek) seekFocus.requestFocus() else playFocus.requestFocus() }
            focusSeek = false
        } else {
            runCatching { rootFocus.requestFocus() }
        }
    }
    LaunchedEffect(Unit) { runCatching { rootFocus.requestFocus() } }
    val show: () -> Unit = {
        visible = true
        poke++
    }

    // Up next (episodes): a card for the last UP_NEXT_S seconds; at the end the next one starts
    // unless the viewer dismissed it (Back).
    var upNextDismissed by remember { mutableStateOf(false) }
    val remainingMs = dur?.let { it - pos }
    val total = dur
    val showUpNext = nextEp != null && !upNextDismissed && st.error == null && menu == null && total != null &&
        total > 2 * UP_NEXT_S * 1000 && (st.ended || (remainingMs != null && remainingMs <= UP_NEXT_S * 1000))
    LaunchedEffect(st.ended) {
        if (!st.ended) return@LaunchedEffect
        if (nextEp != null && !upNextDismissed) playNext() else visible = true
    }

    // Back peels one layer at a time: menu → up next → controls (TV) → leave.
    val back: () -> Unit = {
        when {
            menu != null -> menu = null
            showUpNext -> upNextDismissed = true
            tv && visible && st.error == null && !st.ended -> visible = false
            else -> leave()
        }
    }
    BackHandler(onBack = back)

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onPreviewKeyEvent { e ->
                // Handled here, not only by BackHandler: with a button focused, Compose would spend
                // the key on moving focus out first.
                if (e.key.nativeKeyCode == android.view.KeyEvent.KEYCODE_BACK) {
                    if (e.type == KeyEventType.KeyUp) back()
                    return@onPreviewKeyEvent true
                }
                if (e.type != KeyEventType.KeyDown || menu != null || st.error != null) return@onPreviewKeyEvent false
                // The up-next card has focus: OK plays it.
                if (showUpNext && e.key.nativeKeyCode in OK_KEYS) {
                    playNext()
                    return@onPreviewKeyEvent true
                }
                when (val a = PlayerKeys.map(e.key.nativeKeyCode, visible)) {
                    KeyAction.TogglePlay -> controller.togglePlay()
                    KeyAction.Play -> controller.play()
                    KeyAction.Pause -> controller.pause()
                    is KeyAction.Seek -> {
                        controller.seekBy(a.seconds.toDouble())
                        if (a.focusSeekBar) focusSeek = true
                    }
                    KeyAction.Next -> playNext()
                    KeyAction.Leave -> leave()
                    KeyAction.Wake -> Unit
                    KeyAction.Pass -> {
                        show()
                        return@onPreviewKeyEvent false
                    }
                }
                show()
                true
            }
            .focusRequester(rootFocus)
            .focusable()
            .pointerInput(Unit) {
                detectTapGestures(
                    onTap = { if (menu != null) menu = null else if (visible) visible = false else show() },
                    onDoubleTap = { o ->
                        val third = size.width / 3
                        when {
                            o.x < third -> controller.seekBy(-PlayerKeys.SEEK_S.toDouble())
                            o.x > size.width - third -> controller.seekBy(PlayerKeys.SEEK_S.toDouble())
                            else -> controller.togglePlay()
                        }
                        show()
                    },
                )
            }
            .testTag("player"),
    ) {
        AndroidView(
            factory = { ctx ->
                PlayerView(ctx).apply {
                    layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
                    useController = false
                    isFocusable = false
                    resizeMode = AspectRatioFrameLayout.RESIZE_MODE_FIT
                    setShowBuffering(PlayerView.SHOW_BUFFERING_NEVER)
                    setKeepContentOnPlayerReset(true)
                    this.player = player.exo
                }
            },
            modifier = Modifier.fillMaxSize(),
        )

        if (st.loading && st.error == null) {
            CircularProgressIndicator(Modifier.align(Alignment.Center).testTag("buffering"), color = ReelColors.Accent)
        }
        st.recovering?.let {
            Text(
                it,
                color = ReelColors.Ink,
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.align(Alignment.Center).padding(top = 96.dp),
            )
        }

        val title = heading ?: item?.let { i -> if (i.kind == "episode") series ?: i.title else i.title }.orEmpty()
        val subtitle = item?.takeIf { it.kind == "episode" }?.let { i ->
            listOf(episodeCode(i.seasonNo, i.episodeNo), i.title).filter { it.isNotEmpty() }.joinToString(" · ")
        }
        val s = st.session
        val pills = buildList {
            if (s != null && s.audio.size > 1) add(Pill("Audio", audioValue(s, st.audioIndex), "audio") { menu = Menu.Audio })
            if (s != null && s.subtitles.isNotEmpty()) add(Pill("Subtitles", subtitleValue(s, st.subtitleIndex), "subtitles") { menu = Menu.Subtitles })
            if (s != null && s.qualityOptions.isNotEmpty()) add(Pill("Quality", qualityValue(s, st.quality), "quality") { menu = Menu.Quality })
            if (s != null) add(Pill("Info", null, "info") { menu = Menu.Info })
        }

        AnimatedVisibility(visible = visible && st.error == null, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxSize()) {
                // Top: back (phones), title + episode line, how it's delivered.
                Row(
                    Modifier
                        .fillMaxWidth()
                        .background(TopShade)
                        .safeDrawingPadding()
                        .padding(horizontal = 24.dp, vertical = 16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    if (!tv) CtrlButton(Icons.AutoMirrored.Filled.ArrowBack, "Back", leave)
                    Column(Modifier.weight(1f)) {
                        Text(title, style = MaterialTheme.typography.headlineSmall, color = Color.White, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        subtitle?.let { Text(it, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2, maxLines = 1) }
                    }
                    s?.let { ModeBadge(it, Modifier.testTag("mode")) }
                }
                // Phones: transport in the middle of the picture.
                if (!tv) {
                    Row(
                        Modifier.align(Alignment.Center),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(40.dp),
                    ) {
                        CtrlButton(Icons.Filled.Replay10, "Back 10 seconds", { controller.seekBy(-10.0); show() })
                        CtrlButton(
                            if (st.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                            if (st.playing) "Pause" else "Play",
                            { controller.togglePlay(); show() },
                            modifier = Modifier.testTag("play-pause"),
                            big = true,
                        )
                        CtrlButton(Icons.Filled.Forward10, "Forward 10 seconds", { controller.seekBy(10.0); show() })
                    }
                }
                // Bottom: timeline, times, pills.
                Column(
                    Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .background(BottomShade)
                        .safeDrawingPadding()
                        .padding(horizontal = 32.dp, vertical = 20.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    SeekBar(
                        positionMs = pos,
                        durationMs = dur,
                        bufferedMs = buffered,
                        onSeek = { ms ->
                            controller.seekTo(ms / 1000.0)
                            show()
                        },
                        onNudge = { sec ->
                            controller.seekBy(sec.toDouble())
                            show()
                        },
                        modifier = Modifier.fillMaxWidth().focusRequester(seekFocus),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(formatTime(pos), color = ReelColors.Ink, style = MaterialTheme.typography.bodyMedium.merge(Tabular))
                        Spacer(Modifier.weight(1f))
                        Text(dur?.let { "-" + formatTime(it - pos) }.orEmpty(), color = ReelColors.Ink2, style = MaterialTheme.typography.bodyMedium.merge(Tabular))
                    }
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        if (tv) {
                            PillButton(
                                label = if (st.playing) "Pause" else "Play",
                                value = null,
                                icon = if (st.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                                onClick = { controller.togglePlay(); show() },
                                modifier = Modifier.focusRequester(playFocus).testTag("play-pause"),
                            )
                        }
                        for (pill in pills) PillButton(pill.label, pill.value, onClick = pill.onClick, modifier = Modifier.testTag(pill.tag))
                        Spacer(Modifier.weight(1f))
                        if (nextEp != null) PillButton("Next episode", null, icon = Icons.Filled.SkipNext, onClick = playNext, modifier = Modifier.testTag("next"))
                    }
                }
            }
        }

        menu?.let { m -> PlayerMenu(m, st, controller, onClose = { menu = null }, modifier = Modifier.align(Alignment.CenterEnd)) }

        val n = nextEp
        if (n != null && showUpNext) {
            UpNext(
                next = n,
                seconds = ((dur ?: 0) - pos).coerceAtLeast(0).let { ((it + 999) / 1000).toInt() },
                tv = tv,
                play = playNext,
                modifier = Modifier.align(Alignment.BottomEnd).safeDrawingPadding().padding(32.dp),
            )
        }

        st.error?.let { err ->
            ErrorPanel(err, retry = controller::retry, leave = leave, modifier = Modifier.align(Alignment.Center))
        }
    }
}

@Composable
private fun PlayerMenu(m: Menu, st: VodState, controller: VodController, onClose: () -> Unit, modifier: Modifier) {
    val s = st.session ?: return
    when (m) {
        Menu.Audio -> MenuSheet(
            "Audio",
            s.audio.map { a ->
                val lang = languageName(a.language) ?: a.title ?: "Track ${a.index}"
                val ch = when (a.channels) {
                    1L -> "mono"
                    2L -> "stereo"
                    6L -> "5.1"
                    8L -> "7.1"
                    else -> "${a.channels} ch"
                }
                MenuRow("${a.index}", lang, "${a.codec.uppercase()} $ch" + (a.title?.takeIf { it != lang }?.let { " · $it" } ?: ""), selected = a.index == st.audioIndex)
            },
            onPick = {
                controller.selectAudio(it.toLong())
                onClose()
            },
            modifier = modifier,
        )
        Menu.Subtitles -> MenuSheet(
            "Subtitles",
            listOf(MenuRow("-1", "Off", selected = st.subtitleIndex < 0)) + s.subtitles.map { o ->
                // Picture subtitles the player can't show itself are burned in (forces a transcode).
                val burn = o.url == null && !s.isDirect
                MenuRow(
                    "${o.index}",
                    subtitleLabel(o.language, o.title, o.forced),
                    listOfNotNull(if (o.external) "External" else null, if (burn) "Burned in (transcodes)" else null).joinToString(" · ").ifEmpty { null },
                    selected = o.index == st.subtitleIndex,
                )
            },
            onPick = {
                controller.selectSubtitle(it.toLong())
                onClose()
            },
            modifier = modifier,
        )
        Menu.Quality -> MenuSheet(
            "Quality",
            listOf(MenuRow("auto", "Auto", "Original when it fits, else the best that does", selected = st.quality == "auto")) +
                s.qualityOptions.map { q ->
                    MenuRow(
                        q.id,
                        q.label + if (s.quality == q.id) " · now" else "",
                        if (q.bitrateKbps > 0) "%.1f Mbit/s".format(q.bitrateKbps / 1000.0) else null,
                        selected = st.quality == q.id,
                        enabled = q.available,
                    )
                },
            onPick = {
                controller.selectQuality(it)
                onClose()
            },
            modifier = modifier,
        )
        Menu.Info -> MenuSheet(
            "Playback info",
            rows = listOf(MenuRow("close", "Close")),
            onPick = { onClose() },
            modifier = modifier,
            lines = listOfNotNull(
                modeLabel(s.mode),
                "Video: ${s.decision.video.type} · Audio: ${s.decision.audio.type} · Subtitles: ${s.decision.subtitle.type}",
                s.quality?.let { "Quality: $it" + if (s.bitrateKbps > 0) " · %.1f Mbit/s".format(s.bitrateKbps / 1000.0) else "" },
            ) + s.decision.reasons.map { "• $it" } + listOfNotNull(s.selected.reason.ifBlank { null }?.let { "Tracks: $it" }),
        )
    }
}

/** "Up next in Ns": bottom-right, focused on TV (cream outline), so OK plays it and Back dismisses it. */
@Composable
private fun UpNext(next: ItemSummary, seconds: Int, tv: Boolean, play: () -> Unit, modifier: Modifier) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(next.id) { if (tv) runCatching { focus.requestFocus() } }
    FocusCard(onClick = play, shape = MaterialTheme.shapes.large, modifier = modifier.focusRequester(focus).testTag("up-next")) { focused ->
        Column(
            Modifier
                .background(Scrim)
                .border(if (focused) 0.dp else 1.dp, ReelColors.Ink3, MaterialTheme.shapes.large)
                .padding(horizontal = 24.dp, vertical = 18.dp)
                .widthIn(min = 240.dp, max = 420.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text("Up next in ${seconds}s", style = MaterialTheme.typography.labelLarge.merge(Tabular), color = ReelColors.AccentHi)
            Text(
                listOfNotNull(next.episodeNo?.let { "$it." }, next.title).joinToString(" "),
                style = MaterialTheme.typography.titleMedium,
                color = ReelColors.Ink,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(if (tv) "OK to play · Back to dismiss" else "Tap to play", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        }
    }
}

@Composable
private fun ErrorPanel(err: PlayError, retry: () -> Unit, leave: () -> Unit, modifier: Modifier) {
    val focus = remember { FocusRequester() }
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(err) { runCatching { focus.requestFocus() } }
    Column(
        modifier.background(Scrim, MaterialTheme.shapes.large).padding(28.dp).widthIn(max = 560.dp).testTag("player-error"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(err.title, style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
        Text(err.body, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2)
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            val action = err.action
            if (action != null) {
                Button(
                    onClick = {
                        busy = true
                        scope.launch {
                            runCatching { action.run() }
                            busy = false
                            retry()
                        }
                    },
                    enabled = !busy,
                    modifier = Modifier.focusRequester(focus).testTag("error-action"),
                ) { Text(action.label) }
            }
            if (err.retry) {
                Button(onClick = retry, modifier = (if (action == null) Modifier.focusRequester(focus) else Modifier).testTag("error-retry")) { Text("Try again") }
            }
            OutlinedButton(onClick = leave, modifier = if (action == null && !err.retry) Modifier.focusRequester(focus) else Modifier) { Text("Back") }
        }
    }
}

/** Phones: the player is landscape and immersive (system bars come back with a swipe). */
@Composable
internal fun ImmersiveLandscape() {
    val context = LocalContext.current
    DisposableEffect(Unit) {
        val activity = context.findActivity() ?: return@DisposableEffect onDispose {}
        val before = activity.requestedOrientation
        activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        val insets = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
        insets.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        insets.hide(WindowInsetsCompat.Type.systemBars())
        onDispose {
            insets.show(WindowInsetsCompat.Type.systemBars())
            activity.requestedOrientation = before
        }
    }
}

private fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

private data class Pill(val label: String, val value: String?, val tag: String, val onClick: () -> Unit)

private val OK_KEYS = setOf(android.view.KeyEvent.KEYCODE_DPAD_CENTER, android.view.KeyEvent.KEYCODE_ENTER, android.view.KeyEvent.KEYCODE_NUMPAD_ENTER)

/** "EN 5.1" */
internal fun audioValue(s: app.reel.android.data.DecideResponse, index: Long?): String? {
    val a = s.audio.firstOrNull { it.index == index } ?: return null
    val ch = when (a.channels) {
        1L -> "1.0"
        2L -> "2.0"
        6L -> "5.1"
        8L -> "7.1"
        else -> "${a.channels}ch"
    }
    return listOfNotNull(shortLanguage(a.language), ch).joinToString(" ")
}

/** "Off" / "EN" */
internal fun subtitleValue(s: app.reel.android.data.DecideResponse, index: Long): String {
    if (index < 0) return "Off"
    val o = s.subtitles.firstOrNull { it.index == index } ?: return "Off"
    return shortLanguage(o.language) ?: o.title ?: "On"
}

/** "Auto" or the picked rung's label. */
internal fun qualityValue(s: app.reel.android.data.DecideResponse, quality: String): String =
    if (quality == "auto") "Auto" else s.qualityOptions.firstOrNull { it.id == quality }?.label ?: quality

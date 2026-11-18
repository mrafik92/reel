package app.reel.android.ui.live

import android.os.SystemClock
import android.view.ViewGroup
import androidx.annotation.OptIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.media3.common.util.UnstableApi
import androidx.media3.ui.AspectRatioFrameLayout
import androidx.media3.ui.PlayerView
import app.reel.android.BuildConfig
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.IptvChannel
import app.reel.android.data.Me
import app.reel.android.data.OwnLiveSession
import app.reel.android.data.Program
import app.reel.android.player.LiveController
import app.reel.android.player.LiveState
import app.reel.android.player.PlayError
import app.reel.android.player.ReelPlayer
import app.reel.android.player.Scrim
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.LoadingView
import app.reel.android.ui.common.ReelImage
import app.reel.android.ui.theme.ReelColors
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** The LIVE badge and the programme progress bars (owner-approved design). */
val LiveRose = Color(0xFFFF4F86)

/** Favourite star. */
val StarGold = Color(0xFFFFD23F)

/**
 * The Live TV tab (#191). Phone: a docked player above group chips and the channel list, with a
 * full-screen mode for zapping. TV: the channel list, and a full-screen player with a channel
 * banner and a side list. The provider usually allows one connection, so nothing tunes until the
 * viewer picks a channel, and the session is released on leave, on zap and in the background.
 */
@Composable
fun LiveScreen(user: Me) {
    val graph = LocalAppGraph.current
    val api = graph.session.api ?: return
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val vm: LiveViewModel = viewModel(key = "live") { LiveViewModel(api) }
    val ui by vm.ui.collectAsStateWithLifecycle()
    val owner = "${api.server.url}#${user.id}"

    // This device's last session and channel (DataStore), read before the controller is made.
    var prefs by remember { mutableStateOf<Pair<OwnLiveSession?, Long?>?>(null) }
    LaunchedEffect(owner) {
        prefs = runCatching { graph.devicePrefs.liveSession(owner).first() to graph.devicePrefs.lastChannel.first() }.getOrDefault(null to null)
    }
    val p = prefs
    if (p == null) {
        LoadingView()
        return
    }
    LivePlayerHost(user, ui, vm, owner, startOwn = p.first, lastChannel = p.second, tv = tv)
}

@kotlin.OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)
@Composable
private fun LivePlayerHost(
    user: Me,
    ui: LiveUi,
    vm: LiveViewModel,
    owner: String,
    startOwn: OwnLiveSession?,
    lastChannel: Long?,
    tv: Boolean,
) {
    val graph = LocalAppGraph.current
    val api = graph.session.api ?: return
    val context = LocalContext.current
    val player = remember { ReelPlayer(context, graph.http, "Reel-Android/${BuildConfig.VERSION_NAME}") }
    val controller = remember {
        LiveController(
            source = ApiLiveSource(api, user, startOwn, save = { s -> graph.scope.launch { runCatching { graph.devicePrefs.setLiveSession(owner, s) } } }),
            player = player,
            scope = graph.scope,
            resolve = { url -> api.server.url.resolve(url)?.toString() ?: url },
            now = SystemClock::elapsedRealtime,
            sessionLock = graph.liveSessionLock,
        )
    }
    DisposableEffect(controller) {
        onDispose {
            // Leaving Live TV frees the provider slot.
            controller.close()
            player.release()
        }
    }
    // The background frees the slot too (no background audio in v1); coming back tunes again.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e ->
            when (e) {
                Lifecycle.Event.ON_STOP -> controller.background()
                Lifecycle.Event.ON_START -> controller.foreground()
                else -> Unit
            }
        }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }
    val live by controller.state.collectAsStateWithLifecycle()
    LaunchedEffect(live.session?.channelId) {
        live.session?.channelId?.let { runCatching { graph.devicePrefs.setLastChannel(it) } }
    }

    var visibleIds by remember { mutableStateOf(emptyList<Long>()) }
    val state = LiveScreenState(ui, live, controller, vm, lastChannel, updateVisible = { visibleIds = it })
    val guideIds = (listOfNotNull(state.current?.id) + visibleIds).distinct()
    LaunchedEffect(lifecycle, vm, guideIds) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) { vm.watchGuide(guideIds) }
    }
    // Home's *Live now* row asks for a channel.
    val request by graph.liveRequest.collectAsStateWithLifecycle()
    var requested by remember { mutableStateOf<Long?>(null) }
    LaunchedEffect(request, ui.all.isNotEmpty()) {
        val id = request ?: return@LaunchedEffect
        if (ui.all.isEmpty()) return@LaunchedEffect
        graph.liveRequest.value = null
        if (ui.all.none { it.id == id }) return@LaunchedEffect
        controller.tune(id)
        requested = id
    }

    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(ui.notice) {
        val message = ui.notice ?: return@LaunchedEffect
        snackbar.showSnackbar(message)
        vm.clearNotice()
    }
    Box(Modifier.fillMaxSize()) {
        when (val c = ui.channels) {
            Load.Loading -> LoadingView()
            is Load.Failed -> ErrorView(c.message, onRetry = vm::load)
            is Load.Ready -> if (c.value.isEmpty()) {
                app.reel.android.ui.common.EmptyView("No channels yet. An admin adds an IPTV source in Reel on the web.")
            } else if (tv) {
                TvLive(state, player, openFull = requested)
            } else {
                PhoneLive(state, player)
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }
}

/** Everything the phone and TV layouts read and call. */
class LiveScreenState(
    val ui: LiveUi,
    val live: LiveState,
    val controller: LiveController,
    val vm: LiveViewModel,
    private val lastChannel: Long?,
    val updateVisible: (List<Long>) -> Unit,
) {
    val all get() = ui.all
    val visible get() = ui.visible

    /** The channel on screen: the one playing / tuning, else the last one watched. */
    val current: IptvChannel?
        get() = live.channelId?.let { id -> all.firstOrNull { it.id == id } } ?: LiveLogic.startChannel(visible, all, lastChannel)

    fun nowNext(c: IptvChannel?): Pair<Program?, Program?> = LiveLogic.current(c?.let { ui.now[it.id] }, ui.nowS)

    fun number(c: IptvChannel) = LiveLogic.displayNumber(c, all)

    /** Zap [delta] channels through the list being shown. */
    fun zap(delta: Int) {
        LiveLogic.zapTarget(visible, live.channelId ?: current?.id, delta)?.let { controller.zap(it.id) }
    }
}

/** Feed just the on-screen list rows into the guide request. */
@Composable
internal fun WatchVisibleChannels(list: LazyListState, update: (List<Long>) -> Unit) {
    val latestUpdate by androidx.compose.runtime.rememberUpdatedState(update)
    LaunchedEffect(list) {
        snapshotFlow { list.layoutInfo.visibleItemsInfo.mapNotNull { it.key as? Long } }
            .collect { latestUpdate(it) }
    }
    DisposableEffect(list) { onDispose { latestUpdate(emptyList()) } }
}

/** The ExoPlayer surface (no built-in controls: the screens draw their own). */
@OptIn(UnstableApi::class)
@Composable
fun LiveVideo(player: ReelPlayer, modifier: Modifier = Modifier) {
    AndroidView(
        factory = { ctx ->
            PlayerView(ctx).apply {
                layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
                useController = false
                isFocusable = false
                resizeMode = AspectRatioFrameLayout.RESIZE_MODE_FIT
                setShowBuffering(PlayerView.SHOW_BUFFERING_NEVER)
                setShutterBackgroundColor(android.graphics.Color.BLACK)
                this.player = player.exo
            }
        },
        modifier = modifier,
    )
}

@Composable
fun LiveBadge(modifier: Modifier = Modifier, big: Boolean = false) {
    Box(
        modifier
            .background(LiveRose, RoundedCornerShape(4.dp))
            .padding(horizontal = if (big) 10.dp else 8.dp, vertical = if (big) 4.dp else 3.dp),
    ) {
        Text("LIVE", color = ReelColors.Bg, fontWeight = FontWeight.Bold, fontSize = if (big) 13.sp else 12.sp)
    }
}

/** A thin rose progress bar on a faint track. */
@Composable
fun ProgrammeBar(fraction: Float, modifier: Modifier = Modifier, height: Dp = 3.dp) {
    Box(modifier.height(height).background(ReelColors.Ink.copy(alpha = 0.16f), RoundedCornerShape(2.dp))) {
        if (fraction > 0f) Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(LiveRose, RoundedCornerShape(2.dp)))
    }
}

/** The channel's logo, or its number and an abbreviation when it has none. */
@Composable
fun ChannelTile(c: IptvChannel, number: String, size: Dp, modifier: Modifier = Modifier) {
    Box(
        modifier.size(size).background(ReelColors.Bg2, RoundedCornerShape(10.dp)),
        contentAlignment = Alignment.Center,
    ) {
        if (c.logoUrl != null) {
            ReelImage(
                c.logoUrl,
                contentDescription = null,
                contentScale = androidx.compose.ui.layout.ContentScale.Fit,
                modifier = Modifier.fillMaxSize().padding(6.dp),
            )
        } else {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(number, fontSize = 11.sp, color = ReelColors.Ink.copy(alpha = 0.6f), maxLines = 1)
                Text(LiveLogic.abbreviation(c.name), fontSize = 12.sp, fontWeight = FontWeight.ExtraBold, color = ReelColors.Ink, maxLines = 1)
            }
        }
    }
}

/**
 * A failed tune: what happened, and *Stop the other stream* / *Take over* / *Try again*. An action
 * runs, then the channel tunes again.
 */
@Composable
fun LiveErrorPanel(err: PlayError, retry: () -> Unit, modifier: Modifier = Modifier, compact: Boolean = false, onBack: (() -> Unit)? = null) {
    val focus = remember { FocusRequester() }
    val scope = rememberCoroutineScope()
    var busy by remember(err) { mutableStateOf(false) }
    var actionFailed by remember(err) { mutableStateOf(false) }
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    LaunchedEffect(err) { if (tv) runCatching { focus.requestFocus() } }
    Column(
        modifier
            .background(Scrim, MaterialTheme.shapes.large)
            .padding(if (compact) 14.dp else 28.dp)
            .widthIn(max = 560.dp)
            .testTag("live-error"),
        verticalArrangement = Arrangement.spacedBy(if (compact) 6.dp else 12.dp),
    ) {
        Text(err.title, style = if (compact) MaterialTheme.typography.titleMedium else MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
        Text(
            err.body,
            style = if (compact) MaterialTheme.typography.bodyMedium else MaterialTheme.typography.bodyLarge,
            color = ReelColors.Ink2,
            maxLines = if (compact) 3 else 6,
            overflow = TextOverflow.Ellipsis,
        )
        if (actionFailed) Text("Couldn’t stop the other stream. Try again.", color = ReelColors.Ink)
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            val action = err.action
            if (action != null) {
                Button(
                    onClick = {
                        busy = true
                        actionFailed = false
                        scope.launch {
                            val result = runCatching { action.run() }
                            if (!isActive) return@launch
                            busy = false
                            if (result.isSuccess) retry() else actionFailed = true
                        }
                    },
                    enabled = !busy,
                    modifier = Modifier.focusRequester(focus).testTag("live-error-action"),
                ) { Text(action.label, maxLines = 1) }
            }
            if (err.retry) {
                val m = if (action == null) Modifier.focusRequester(focus) else Modifier
                if (action == null) {
                    Button(onClick = retry, enabled = !busy, modifier = m.testTag("live-error-retry")) { Text("Try again") }
                } else {
                    OutlinedButton(onClick = retry, enabled = !busy, modifier = m.testTag("live-error-retry")) { Text("Try again") }
                }
            }
            if (onBack != null) {
                OutlinedButton(onClick = onBack, modifier = if (action == null && !err.retry) Modifier.focusRequester(focus) else Modifier) { Text("Back") }
            }
        }
    }
}

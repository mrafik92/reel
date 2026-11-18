package app.reel.android.ui.vod

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.graphics.Color
import app.reel.android.LocalAppGraph
import app.reel.android.data.AuthState
import app.reel.android.data.ItemDetail
import app.reel.android.data.ReelError
import app.reel.android.data.VodMovieDetail
import app.reel.android.data.VodSeriesDetail
import app.reel.android.player.VodPlayer
import app.reel.android.player.StartupTiming
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.theme.ReelColors
import kotlinx.coroutines.flow.first

/** What the player shows for an IPTV title (a stand-in item: no library ids). */
private data class VodTarget(val item: ItemDetail, val heading: String?, val runtimeS: Long?)

/**
 * An IPTV movie or episode in the VOD player (`POST /vod/play`). The provider usually allows one
 * connection: busy answers explain who holds it and offer *Stop the other stream* / *Take over*.
 */
@Composable
fun VodPlayerScreen(kind: String, id: Long, seriesId: Long?, startS: Double, tapMs: Long = 0) {
    val graph = LocalAppGraph.current
    val api = graph.session.api ?: return
    var target by remember { mutableStateOf<Result<VodTarget>?>(null) }
    var deviceId by remember { mutableStateOf<String?>(null) }
    var me by remember { mutableStateOf<app.reel.android.data.Me?>(null) }
    var quality by remember { mutableStateOf("auto") }
    var attempt by remember { mutableIntStateOf(0) }
    val timing = remember(attempt, tapMs) {
        StartupTiming.begin(vod = true, tapMs = if (attempt == 0) tapMs else 0)?.apply { mark(StartupTiming.Stage.route) }
    }
    DisposableEffect(timing) { onDispose { timing?.finish(StartupTiming.Outcome.canceled) } }
    LaunchedEffect(attempt) {
        me = (graph.session.state.value as? AuthState.SignedIn)?.user
        quality = runCatching { graph.devicePrefs.quality.first() }.getOrNull() ?: "auto"
        deviceId = runCatching { graph.session.deviceId() }.getOrNull() ?: ""
        target = runCatching {
            if (kind == "movie") {
                val m = api.get<VodMovieDetail>("/vod/movies/$id")
                VodTarget(ItemDetail(id = -id, kind = "movie", title = m.title, year = m.year, posterUrl = m.posterUrl), null, m.runtimeS)
            } else {
                val s = api.get<VodSeriesDetail>("/vod/series/${seriesId ?: error("This episode link is missing its series.")}")
                val (season, ep) = s.seasons.firstNotNullOfOrNull { se -> se.episodes.firstOrNull { it.id == id }?.let { se to it } }
                    ?: throw IllegalStateException("This episode isn't on your IPTV provider anymore.")
                val generic = Regex("""^(episode|ep\.?|الحلقة)\s*\d+$""", RegexOption.IGNORE_CASE).matches(ep.title.trim())
                VodTarget(
                    ItemDetail(
                        id = -id,
                        kind = "episode",
                        title = if (generic) "Episode ${ep.episodeNo}" else ep.title,
                        seasonNo = season.seasonNo,
                        episodeNo = ep.episodeNo,
                    ),
                    s.title,
                    ep.runtimeS,
                )
            }
        }.onFailure {
            if (it !is kotlinx.coroutines.CancellationException) timing?.finish(StartupTiming.Outcome.failed)
        }
    }
    val t = target
    val dev = deviceId
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        when {
            t == null || dev == null -> CircularProgressIndicator(Modifier.align(Alignment.Center), color = ReelColors.Accent)
            t.isFailure -> ErrorView(
                (t.exceptionOrNull() as? ReelError)?.userMessage ?: t.exceptionOrNull()?.message ?: "Couldn't open this IPTV title",
                onRetry = { attempt++ },
            )
            else -> {
                val v = t.getOrThrow()
                val source = remember(kind, id) { VodSource(api, kind, id, dev.ifEmpty { null }, me) }
                VodPlayer(
                    source = source,
                    api = api,
                    item = v.item,
                    durationS = v.runtimeS?.toDouble()?.takeIf { it > 0 },
                    startS = startS,
                    nextEnabled = false,
                    heading = v.heading,
                    initialQuality = quality,
                    startup = timing,
                )
            }
        }
    }
}

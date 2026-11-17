package app.reel.android.player

import android.content.Context
import android.net.Uri
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.Tracks
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import okhttp3.OkHttpClient

/**
 * The app's one player (#190): ExoPlayer with an OkHttp data source, behind [PlayerPort].
 * The VOD controller drives it now; Live TV (#191) reuses it for the `passthrough` MPEG-TS stream
 * ([PlayItem.MIME_TS]). The screen shows [exo] in a media3-ui `PlayerView`.
 *
 * - [http] must be the **plain** client, not the API's: media URLs are signed, and the bearer
 *   token never goes to them.
 * - Track picks are kept as wishes and applied whenever the tracks change, so a pick made before
 *   the media is ready (or kept across a reload) still lands.
 */
@OptIn(UnstableApi::class)
class ReelPlayer(context: Context, http: OkHttpClient, userAgent: String) : PlayerPort {
    val exo: ExoPlayer

    override var listener: PlayerListener? = null

    private var wantedText: TextChoice = TextChoice.Off
    private var wantedAudio: Int? = null
    private var reportedDuration = false

    init {
        val network = OkHttpDataSource.Factory(http).setUserAgent(userAgent)
        val renderers = DefaultRenderersFactory(context)
            // A device whose preferred decoder fails (common on cheap TV sticks) tries the next one.
            .setEnableDecoderFallback(true)
        exo = ExoPlayer.Builder(context, renderers)
            .setMediaSourceFactory(DefaultMediaSourceFactory(DefaultDataSource.Factory(context, network)))
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MOVIE).build(),
                /* handleAudioFocus = */ true,
            )
            .setHandleAudioBecomingNoisy(true)
            .setSeekBackIncrementMs(10_000)
            .setSeekForwardIncrementMs(30_000)
            .build()
        exo.addListener(Events())
    }

    override fun load(item: PlayItem) {
        reportedDuration = false
        wantedAudio = null
        wantedText = TextChoice.Off
        val subs = item.subtitles.map { s ->
            MediaItem.SubtitleConfiguration.Builder(Uri.parse(s.url))
                .setId(s.id)
                .setMimeType(MimeTypes.TEXT_VTT)
                .setLanguage(s.language)
                .setLabel(s.label)
                .build()
        }
        val media = MediaItem.Builder()
            .setUri(item.url)
            .apply { item.mimeType?.let { setMimeType(it) } }
            .setSubtitleConfigurations(subs)
            .build()
        exo.setMediaItem(media, item.startMs.coerceAtLeast(0))
        exo.playWhenReady = item.autoplay
        exo.prepare()
    }

    override fun play() {
        if (exo.playbackState == Player.STATE_IDLE) exo.prepare()
        exo.play()
    }

    override fun pause() = exo.pause()

    override fun seekTo(positionMs: Long) = exo.seekTo(positionMs.coerceAtLeast(0))

    override fun stop() = exo.stop()

    fun release() {
        listener = null
        exo.release()
    }

    override val positionMs: Long get() = exo.currentPosition

    override val durationMs: Long? get() = exo.duration.takeIf { it != C.TIME_UNSET && it > 0 }

    override val playWhenReady: Boolean get() = exo.playWhenReady

    override fun audioTrackCount(): Int = exo.currentTracks.groups.count { it.type == C.TRACK_TYPE_AUDIO }

    override fun selectAudio(position: Int) {
        wantedAudio = position
        applyTracks()
    }

    override fun selectText(choice: TextChoice) {
        wantedText = choice
        applyTracks()
    }

    override fun hasEmbeddedText(position: Int): Boolean =
        embeddedText(exo.currentTracks).getOrNull(position)?.isSupported == true

    private fun embeddedText(tracks: Tracks): List<Tracks.Group> =
        tracks.groups.filter { it.type == C.TRACK_TYPE_TEXT && !isSidecar(it) }

    private fun isSidecar(g: Tracks.Group): Boolean = sidecarId(g)?.startsWith(SIDECAR_PREFIX) == true

    /** Sidecar ids may come back prefixed by the merging source (`1:reel-sub-3`). */
    private fun sidecarId(g: Tracks.Group): String? = g.getTrackFormat(0).id?.substringAfterLast(':')

    private fun applyTracks() {
        val tracks = exo.currentTracks
        if (tracks.groups.isEmpty()) return
        val b = exo.trackSelectionParameters.buildUpon()

        val text = when (val c = wantedText) {
            TextChoice.Off -> null
            is TextChoice.Sidecar -> tracks.groups.firstOrNull { it.type == C.TRACK_TYPE_TEXT && sidecarId(it) == c.id }
            is TextChoice.Embedded -> embeddedText(tracks).getOrNull(c.position)
        }
        b.clearOverridesOfType(C.TRACK_TYPE_TEXT)
        if (text == null) {
            b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
        } else {
            b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
            b.setOverrideForType(TrackSelectionOverride(text.mediaTrackGroup, 0))
        }

        val audio = wantedAudio?.let { pos -> tracks.groups.filter { it.type == C.TRACK_TYPE_AUDIO }.getOrNull(pos) }
        if (audio != null && audio.isSupported) {
            b.setOverrideForType(TrackSelectionOverride(audio.mediaTrackGroup, 0))
        }
        val next = b.build()
        if (next != exo.trackSelectionParameters) exo.trackSelectionParameters = next
    }

    private inner class Events : Player.Listener {
        override fun onPlaybackStateChanged(state: Int) {
            val l = listener ?: return
            when (state) {
                Player.STATE_BUFFERING -> l.onState(PlayerState.Buffering)
                Player.STATE_READY -> {
                    l.onState(PlayerState.Ready)
                    val d = durationMs
                    if (!reportedDuration && d != null) {
                        reportedDuration = true
                        l.onDuration(d)
                    }
                }
                Player.STATE_ENDED -> l.onState(PlayerState.Ended)
                else -> l.onState(PlayerState.Idle)
            }
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            listener?.onIsPlaying(isPlaying)
        }

        override fun onTracksChanged(tracks: Tracks) {
            applyTracks()
            listener?.onTracks()
        }

        override fun onPlayerError(error: PlaybackException) {
            // 4xxx: decoding problems (init, capabilities, format).
            listener?.onFatalError(describe(error), decoder = error.errorCode / 1000 == 4)
        }
    }

    companion object {
        const val SIDECAR_PREFIX = "reel-sub-"

        fun describe(e: PlaybackException): String = when (e.errorCode) {
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT,
            -> "The connection to the server dropped"
            PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS -> "The server stopped sending the stream"
            PlaybackException.ERROR_CODE_DECODER_INIT_FAILED,
            PlaybackException.ERROR_CODE_DECODER_QUERY_FAILED,
            PlaybackException.ERROR_CODE_DECODING_FAILED,
            PlaybackException.ERROR_CODE_DECODING_FORMAT_UNSUPPORTED,
            PlaybackException.ERROR_CODE_DECODING_FORMAT_EXCEEDS_CAPABILITIES,
            -> "This device can't decode the stream"
            else -> "The stream failed (${e.errorCodeName})"
        }
    }
}

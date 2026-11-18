package app.reel.android.player

/**
 * What the playback controllers need from a player. [ReelPlayer] implements it on ExoPlayer; the
 * unit tests use a fake. Keeping the controllers on this interface makes the session logic
 * (decide, progress, re-decide) testable without a device.
 */
interface PlayerPort {
    /** Receives the player's events (on the main thread). */
    var listener: PlayerListener?

    /** Replace whatever is playing with [item]. */
    fun load(item: PlayItem)

    fun play()

    fun pause()

    fun seekTo(positionMs: Long)

    /** Stop and drop the media (the player stays usable for another [load]). */
    fun stop()

    /** Current position in ms (absolute: HLS playlists from Reel cover the whole file). */
    val positionMs: Long

    /** Duration in ms, or `null` while unknown. */
    val durationMs: Long?

    /** Playing, or about to (not paused by the viewer). */
    val playWhenReady: Boolean

    /** Audio tracks of the loaded media, in container order (0 before tracks are known). */
    fun audioTrackCount(): Int

    /** Play the audio track at [position] (container order). Applied once tracks are known. */
    fun selectAudio(position: Int)

    /** The subtitle track to show. Applied once tracks are known, and again after a reload. */
    fun selectText(choice: TextChoice)

    /** The loaded media has an embedded (non-sidecar) text track at [position] that can be shown. */
    fun hasEmbeddedText(position: Int): Boolean
}

interface PlayerListener {
    /** The media is ready / buffering / ended. */
    fun onState(state: PlayerState) {}

    /** Actually playing (ready and not paused) changed. */
    fun onIsPlaying(playing: Boolean) {}

    /** The duration became known (ms). */
    fun onDuration(durationMs: Long) {}

    /**
     * The player gave up (its own retries are spent). The media must be loaded again.
     * [decoder]: this device can't decode the media (a codec / profile it lacks), so loading the
     * same media again won't help.
     */
    fun onFatalError(message: String, decoder: Boolean = false) {}

    /** The tracks of the loaded media changed (menus may want to refresh). */
    fun onTracks() {}
}

enum class PlayerState { Idle, Buffering, Ready, Ended }

/**
 * One thing to play.
 *
 * @param url absolute, signed media URL. It is fetched **without** the bearer token.
 * @param mimeType e.g. [MIME_HLS] or [MIME_TS]; `null` lets ExoPlayer sniff (progressive files).
 * @param subtitles sidecar WebVTT tracks, shown only when picked with [TextChoice.Sidecar].
 */
data class PlayItem(
    val url: String,
    val mimeType: String? = null,
    val startMs: Long = 0,
    val autoplay: Boolean = true,
    val subtitles: List<SidecarText> = emptyList(),
    val title: String? = null,
    val startup: StartupTiming? = null,
) {
    companion object {
        const val MIME_HLS = "application/x-mpegURL"
        const val MIME_TS = "video/mp2t"
    }
}

/** A sidecar WebVTT subtitle. [id] is matched by [TextChoice.Sidecar]. */
data class SidecarText(val id: String, val url: String, val language: String?, val label: String)

sealed interface TextChoice {
    data object Off : TextChoice

    data class Sidecar(val id: String) : TextChoice

    /** The [position]-th text track inside the media itself (e.g. PGS in a direct-played MKV). */
    data class Embedded(val position: Int) : TextChoice
}

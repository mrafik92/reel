package app.reel.android.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

// Wire models for the parts of server/openapi.json the app uses. Unknown fields are ignored
// (see ReelJson), so the server can add fields without breaking older app builds. Fields the
// server always sends still get defaults where a missing value is harmless.
// ModelsTest checks every field name here against the OpenAPI schemas, so they can't drift silently.

/** `GET /health`. Version, git SHA and build date only come back for signed-in callers. */
@Serializable
data class Health(
    val status: String,
    val version: String? = null,
    @SerialName("git_sha") val gitSha: String? = null,
    @SerialName("build_date") val buildDate: String? = null,
)

@Serializable
data class SetupStatus(@SerialName("needs_setup") val needsSetup: Boolean)

/** `POST /auth/login`. Signing in again with the same `device_id` replaces that install's session. */
@Serializable
data class LoginBody(
    val username: String,
    val password: String,
    val client: String = "android",
    @SerialName("device_id") val deviceId: String? = null,
    @SerialName("device_name") val deviceName: String? = null,
)

@Serializable
data class Permissions(
    @SerialName("iptv_access") val iptvAccess: Boolean = false,
    @SerialName("vod_access") val vodAccess: Boolean = false,
    @SerialName("can_request") val canRequest: Boolean = false,
    @SerialName("can_cast") val canCast: Boolean = false,
)

@Serializable
data class Me(
    val id: Long,
    val username: String,
    val role: String,
    val permissions: Permissions = Permissions(),
    @SerialName("direct_live") val directLive: Boolean = false,
    /** Libraries this user may see; `null` = all. */
    @SerialName("library_ids") val libraryIds: List<Long>? = null,
) {
    val isAdmin get() = role == "admin"
    val canWatchLive get() = isAdmin || permissions.iptvAccess

    /** IPTV movies and series (#193). */
    val canWatchVod get() = isAdmin || permissions.vodAccess
}

/** Error responses: `{"error": "..."}` (some carry extra fields, e.g. `takeover`). */
@Serializable
data class ErrorBody(val error: String = "")

@Serializable
data class LoginResponse(val token: String, @SerialName("expires_at") val expiresAt: Long, val user: Me)

@Serializable
data class ItemSummary(
    val id: Long,
    val kind: String,
    val title: String,
    val played: Boolean = false,
    @SerialName("added_at") val addedAt: Long = 0,
    val year: Long? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    @SerialName("backdrop_url") val backdropUrl: String? = null,
    @SerialName("parent_id") val parentId: Long? = null,
    @SerialName("season_no") val seasonNo: Long? = null,
    @SerialName("episode_no") val episodeNo: Long? = null,
    val progress: Double? = null,
    val rating: Double? = null,
    @SerialName("runtime_s") val runtimeS: Long? = null,
    val genres: List<String>? = null,
)

@Serializable
data class HomeRow(@SerialName("library_id") val libraryId: Long, val title: String, val items: List<ItemSummary>)

@Serializable
data class Home(
    @SerialName("continue_watching") val continueWatching: List<ItemSummary>,
    @SerialName("recently_added") val recentlyAdded: List<ItemSummary>,
    val rows: List<HomeRow>,
)

@Serializable
data class Library(
    val id: Long,
    val name: String,
    /** `movies` | `shows`. */
    val kind: String,
    @SerialName("item_count") val itemCount: Long = 0,
)

@Serializable
data class ItemPage(val items: List<ItemSummary>, @SerialName("next_cursor") val nextCursor: String? = null)

@Serializable
data class AudioStream(
    val index: Long,
    val codec: String,
    val channels: Long = 2,
    val default: Boolean = false,
    val language: String? = null,
    val title: String? = null,
)

@Serializable
data class SubtitleStream(
    val index: Long,
    val codec: String,
    val forced: Boolean = false,
    val external: Boolean = false,
    val language: String? = null,
    val title: String? = null,
)

@Serializable
data class VideoStream(
    val codec: String,
    val width: Long,
    val height: Long,
    /** `none` | `hdr10` | `hlg` | `dv`. */
    val hdr: String = "none",
    @SerialName("bit_depth") val bitDepth: Long = 8,
)

@Serializable
data class MediaFile(
    val id: Long,
    val container: String,
    @SerialName("duration_s") val durationS: Double,
    val size: Long,
    /** `/api/v1/direct/{id}` (signed by decide; the app plays decide's `url`, not this). */
    val url: String = "",
    val video: VideoStream? = null,
    val audio: List<AudioStream> = emptyList(),
    val subtitles: List<SubtitleStream> = emptyList(),
    /** Multi-episode files: every episode item stored in this file. */
    @SerialName("item_ids") val itemIds: List<Long> = emptyList(),
)

@Serializable
data class CastMember(val name: String, val character: String? = null, @SerialName("photo_url") val photoUrl: String? = null)

@Serializable
data class ItemDetail(
    val id: Long,
    val kind: String,
    val title: String,
    val played: Boolean = false,
    @SerialName("added_at") val addedAt: Long = 0,
    val year: Long? = null,
    val overview: String? = null,
    val rating: Double? = null,
    @SerialName("content_rating") val contentRating: String? = null,
    @SerialName("runtime_s") val runtimeS: Long? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    @SerialName("backdrop_url") val backdropUrl: String? = null,
    @SerialName("parent_id") val parentId: Long? = null,
    @SerialName("season_no") val seasonNo: Long? = null,
    @SerialName("episode_no") val episodeNo: Long? = null,
    val progress: Double? = null,
    val genres: List<String> = emptyList(),
    val cast: List<CastMember> = emptyList(),
    @SerialName("children_count") val childrenCount: Long = 0,
    val files: List<MediaFile> = emptyList(),
    @SerialName("tmdb_id") val tmdbId: Long? = null,
) {
    val playable get() = files.isNotEmpty() && (kind == "movie" || kind == "episode")
}

/** `POST /playback/{file}/decide`. The server's `android_exoplayer` preset describes ExoPlayer. */
@Serializable
data class DecideRequest(
    val preset: String = "android_exoplayer",
    @SerialName("device_id") val deviceId: String? = null,
    @SerialName("item_id") val itemId: Long? = null,
    @SerialName("start_s") val startS: Double? = null,
    @SerialName("audio_index") val audioIndex: Long? = null,
    @SerialName("subtitle_index") val subtitleIndex: Long? = null,
    val quality: String? = null,
    @SerialName("max_bitrate_kbps") val maxBitrateKbps: Int? = null,
)

@Serializable
data class SubtitleOption(
    val index: Long,
    @SerialName("stream_id") val streamId: Long,
    val forced: Boolean = false,
    val external: Boolean = false,
    val language: String? = null,
    val title: String? = null,
    val url: String? = null,
)

@Serializable
data class Selected(
    val reason: String = "",
    @SerialName("audio_index") val audioIndex: Long? = null,
    @SerialName("subtitle_index") val subtitleIndex: Long? = null,
)

/** One stream's action in a [PlaybackDecision]. Only `type` is read; encode details are ignored. */
@Serializable
data class StreamAction(
    /** Video / audio: `none` | `copy` | `encode`. Subtitles: `none` | `sidecar` | `burn_in` | `native`. */
    val type: String = "none",
)

/** How the server delivers each stream, and why (shown as the player's *why it transcodes*). */
@Serializable
data class PlaybackDecision(
    val mode: String = "",
    val video: StreamAction = StreamAction(),
    val audio: StreamAction = StreamAction(),
    val subtitle: StreamAction = StreamAction(),
    val reasons: List<String> = emptyList(),
    @SerialName("audio_index") val audioIndex: Long? = null,
    @SerialName("subtitle_index") val subtitleIndex: Long? = null,
)

/** An entry of the quality menu (#87). `id`: `original` | `1080p` | `720p` | `480p`. */
@Serializable
data class QualityOption(
    val id: String,
    val label: String,
    @SerialName("bitrate_kbps") val bitrateKbps: Int = 0,
    val available: Boolean = true,
)

@Serializable
data class DecideResponse(
    @SerialName("session_id") val sessionId: String,
    /** `direct_play` | `remux` | `audio_transcode` | `transcode`. */
    val mode: String,
    val url: String,
    val decision: PlaybackDecision = PlaybackDecision(),
    val subtitles: List<SubtitleOption> = emptyList(),
    val audio: List<AudioStream> = emptyList(),
    val selected: Selected = Selected(),
    @SerialName("bitrate_kbps") val bitrateKbps: Int = 0,
    /** Resolved quality: `original` or a ladder rung. */
    val quality: String? = null,
    @SerialName("quality_options") val qualityOptions: List<QualityOption> = emptyList(),
) {
    val isHls get() = url.substringBefore('?').endsWith(".m3u8")
    val isDirect get() = mode == "direct_play"
    val burnsSubtitles get() = decision.subtitle.type == "burn_in"
}

@Serializable
data class ProgressRequest(@SerialName("position_s") val positionS: Double, val paused: Boolean)

@Serializable
data class IptvChannel(
    val id: Long,
    val name: String,
    val number: Long? = null,
    val group: String? = null,
    val favorite: Boolean = false,
    @SerialName("logo_url") val logoUrl: String? = null,
    @SerialName("has_epg") val hasEpg: Boolean = false,
    val catchup: Boolean = false,
)

@Serializable
data class Program(val start: Long, val stop: Long, val title: String)

@Serializable
data class NowNext(@SerialName("channel_id") val channelId: Long, val now: Program? = null, val next: Program? = null)

/**
 * Native players take the provider's MPEG-TS as is (`passthrough`, like webOS / Tizen), so the
 * server copies the stream and zapping stays fast. The preset is sent too, for servers that
 * fall back to HLS.
 */
@Serializable
data class TuneBody(
    val mode: String = "passthrough",
    val preset: String = "android_exoplayer",
    val takeover: Boolean? = null,
)

@Serializable
data class TuneResponse(
    @SerialName("session_id") val sessionId: String,
    val mode: String,
    val url: String,
    @SerialName("channel_id") val channelId: Long,
    @SerialName("channel_name") val channelName: String,
)

/** `GET /genres?library_id=`: a genre and how many movies/shows carry it. */
@Serializable
data class GenreCount(val name: String, val count: Long = 0)

/** A person in `GET /search/all`: library cast/crew merged with TMDB people. */
@Serializable
data class PersonHit(
    @SerialName("tmdb_id") val tmdbId: Long,
    val name: String,
    @SerialName("known_for") val knownFor: List<String> = emptyList(),
    @SerialName("in_library_count") val inLibraryCount: Long = 0,
    @SerialName("known_for_department") val knownForDepartment: String? = null,
    @SerialName("photo_url") val photoUrl: String? = null,
)

/**
 * `GET /search/all?q=`: movies and shows (title matches, then cast/crew matches), episodes and
 * people. The server also sends `discover` and `vod`; the app doesn't show those yet.
 */
@Serializable
data class SearchAll(
    val library: List<ItemSummary> = emptyList(),
    val episodes: List<ItemSummary> = emptyList(),
    val people: List<PersonHit> = emptyList(),
) {
    val isEmpty get() = library.isEmpty() && episodes.isEmpty() && people.isEmpty()
}

/**
 * `GET/PUT /me/preferences`: how Reel picks audio and subtitles at Play, saved to the account (so
 * the web and every device share them). Quality is per device instead (see [DevicePrefs]).
 */
@Serializable
data class PlaybackPrefs(
    /** Ordered; `"original"` = the title's original language. */
    @SerialName("audio_languages") val audioLanguages: List<String> = listOf("original"),
    @SerialName("subtitle_languages") val subtitleLanguages: List<String> = listOf("en"),
    /** `off` | `forced_only` | `auto` | `always`. */
    @SerialName("subtitle_mode") val subtitleMode: String = "auto",
    @SerialName("prefer_sdh") val preferSdh: Boolean = false,
    @SerialName("burn_in_image_subs") val burnInImageSubs: Boolean = false,
)

/** `GET /me/sessions`: a signed-in device. */
@Serializable
data class SessionInfo(
    val id: Long,
    @SerialName("last_seen") val lastSeen: Long,
    /** The session making the request (this app). */
    val current: Boolean = false,
    val client: String? = null,
    @SerialName("device_id") val deviceId: String? = null,
    @SerialName("device_name") val deviceName: String? = null,
    @SerialName("expires_at") val expiresAt: Long? = null,
    val ip: String? = null,
)

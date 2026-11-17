package app.reel.android.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

// IPTV VOD (#193): the provider's movie and series catalogue. Same rules as Models.kt
// (ModelsTest checks every field against server/openapi.json).

@Serializable
data class VodCard(
    val id: Long,
    /** `movie` | `series`. */
    val kind: String,
    val title: String,
    val year: Long? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    val rating: Double? = null,
    val progress: Double? = null,
    val played: Boolean? = null,
    val genres: List<String>? = null,
    @SerialName("category_id") val categoryId: Long? = null,
    @SerialName("library_item_id") val libraryItemId: Long? = null,
    @SerialName("tmdb_id") val tmdbId: Long? = null,
)

/** "See all" of a lane: a query string for `GET /vod/movies` / `/vod/series`. */
@Serializable
data class VodSeeAll(val type: String, val query: String)

@Serializable
data class VodLane(
    val key: String,
    val title: String,
    val kind: String = "",
    val items: List<VodCard> = emptyList(),
    val subtitle: String? = null,
    @SerialName("see_all") val seeAll: VodSeeAll? = null,
)

@Serializable
data class VodPage(val items: List<VodCard> = emptyList(), @SerialName("next_cursor") val nextCursor: String? = null)

@Serializable
data class VodMovieDetail(
    val id: Long,
    val kind: String = "movie",
    val title: String,
    val year: Long? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    @SerialName("backdrop_url") val backdropUrl: String? = null,
    val rating: Double? = null,
    val progress: Double? = null,
    val played: Boolean? = null,
    val genres: List<String>? = null,
    val overview: String? = null,
    val director: String? = null,
    val container: String? = null,
    @SerialName("runtime_s") val runtimeS: Long? = null,
    val cast: List<CastMember> = emptyList(),
)

@Serializable
data class VodEpisode(
    val id: Long,
    @SerialName("episode_no") val episodeNo: Long,
    val title: String,
    val overview: String? = null,
    @SerialName("still_url") val stillUrl: String? = null,
    @SerialName("runtime_s") val runtimeS: Long? = null,
    val progress: Double? = null,
    val played: Boolean? = null,
)

@Serializable
data class VodSeason(
    @SerialName("season_no") val seasonNo: Long,
    val name: String? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    val episodes: List<VodEpisode> = emptyList(),
)

@Serializable
data class VodSeriesDetail(
    val id: Long,
    val kind: String = "series",
    val title: String,
    val year: Long? = null,
    @SerialName("poster_url") val posterUrl: String? = null,
    @SerialName("backdrop_url") val backdropUrl: String? = null,
    val rating: Double? = null,
    val genres: List<String>? = null,
    val overview: String? = null,
    val cast: List<CastMember> = emptyList(),
    val seasons: List<VodSeason> = emptyList(),
)

/** `POST /vod/play`: answers like decide. `takeover` (admins) stops other users' provider streams. */
@Serializable
data class VodPlayRequest(
    /** `movie` | `episode`. */
    val kind: String,
    val id: Long,
    val preset: String = "android_exoplayer",
    @SerialName("device_id") val deviceId: String? = null,
    @SerialName("start_s") val startS: Double? = null,
    @SerialName("audio_index") val audioIndex: Long? = null,
    @SerialName("subtitle_index") val subtitleIndex: Long? = null,
    val quality: String? = null,
    val takeover: Boolean? = null,
)

package app.reel.android.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

// Live TV (#191): channels, groups, now/next, tune and stop. Wire models for the channel list and
// tune live in Models.kt; these are the ones only Live TV needs. ModelsTest checks them against
// server/openapi.json.

@Serializable
data class IptvGroup(val name: String, val count: Long = 0)

@Serializable
data class EpgChannel(@SerialName("channel_id") val channelId: Long, val programs: List<Program> = emptyList())

@Serializable
data class EpgGrid(val from: Long, val to: Long, val channels: List<EpgChannel> = emptyList())

/** Enabled channels, by channel number then name. */
suspend fun ReelApi.channels(): List<IptvChannel> = get("/iptv/channels")

suspend fun ReelApi.channelGroups(): List<IptvGroup> = get("/iptv/groups")

/** Now and next per channel; [ids] empty = the server's first 500 channels. */
suspend fun ReelApi.nowNext(ids: Collection<Long> = emptyList()): List<NowNext> =
    get("/iptv/now", mapOf("channels" to ids.joinToString(",").ifEmpty { null }))

/** One channel's programmes from [fromS] to [toS] (unix seconds): the TV banner's "next two". */
suspend fun ReelApi.schedule(channelId: Long, fromS: Long, toS: Long): List<Program> =
    get<EpgGrid>("/iptv/epg", mapOf("from" to fromS, "to" to toS, "channels" to channelId))
        .channels.firstOrNull { it.channelId == channelId }?.programs.orEmpty()

suspend fun ReelApi.setFavorite(channelId: Long, on: Boolean) {
    if (on) postEmpty("/iptv/channels/$channelId/favorite") else delete("/iptv/channels/$channelId/favorite")
}

/** Reserve a provider slot and get the signed `stream.ts` URL (`passthrough`, ExoPlayer preset). */
suspend fun ReelApi.tune(channelId: Long, takeover: Boolean = false): TuneResponse =
    post<TuneBody, TuneResponse>("/iptv/channels/$channelId/tune", TuneBody(takeover = if (takeover) true else null))

/** Free the provider slot. */
suspend fun ReelApi.stopLive(sessionId: String) = delete("/iptv/sessions/$sessionId")

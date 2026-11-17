package app.reel.android.player

import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary

/** Where to start [item]: its saved position, or 0 when it's unwatched or (nearly) finished. */
fun resumePositionS(item: ItemDetail): Double {
    val file = item.files.firstOrNull() ?: return 0.0
    val p = item.progress ?: return 0.0
    return if (p > 0 && p < RESUME_MAX_PROGRESS) p * file.durationS else 0.0
}

/** Progress at or past this counts as watched: play from the start. */
const val RESUME_MAX_PROGRESS = 0.95

/** The episode after [current] in its season (by episode number), or `null`. */
fun nextEpisode(current: ItemDetail, siblings: List<ItemSummary>): ItemSummary? {
    if (current.kind != "episode") return null
    val cur = current.episodeNo ?: return null
    return siblings
        .filter { it.kind == "episode" && it.id != current.id && (it.episodeNo ?: -1) > cur }
        .minByOrNull { it.episodeNo ?: Long.MAX_VALUE }
}

/** "S1 E2" (or "E2" / ""). */
fun episodeCode(season: Long?, episode: Long?): String = listOfNotNull(season?.let { "S$it" }, episode?.let { "E$it" }).joinToString(" ")

/** Human label for a playback mode. */
fun modeLabel(mode: String): String = when (mode) {
    "direct_play" -> "Direct play"
    "remux" -> "Direct stream (remux)"
    "audio_transcode" -> "Audio converted"
    "transcode" -> "Transcoding"
    else -> mode
}

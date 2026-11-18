package app.reel.android.ui.item

import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.MediaFile
import java.util.Locale
import kotlin.math.roundToLong

/** Pure rules of the item page (#188), shared with the web where it has the same rule. */
object ItemLogic {
    /** Progress past this counts as finished: Play starts over (same as the web player). */
    const val FINISHED = 0.95

    /** Progress below this is noise (a second of playback), not a resume point. */
    const val STARTED = 0.005

    /**
     * Where Play starts, in seconds: the saved position (progress × duration), or 0 when the title
     * is watched, barely started, nearly finished, or the duration is unknown.
     */
    fun resumeAt(progress: Double?, durationS: Double?, played: Boolean): Double {
        if (played || progress == null || durationS == null || durationS <= 0) return 0.0
        if (progress < STARTED || progress >= FINISHED) return 0.0
        return progress * durationS
    }

    /** `12:05` or `1:02:03`. */
    fun clock(seconds: Double): String {
        val t = seconds.coerceAtLeast(0.0).roundToLong()
        val h = t / 3600
        val m = (t % 3600) / 60
        val s = t % 60
        return if (h > 0) String.format(Locale.ROOT, "%d:%02d:%02d", h, m, s) else String.format(Locale.ROOT, "%d:%02d", m, s)
    }

    /** `1 h 52 min`, `45 min`. */
    fun runtime(seconds: Long): String {
        val min = (seconds + 30) / 60
        return when {
            min < 60 -> "$min min"
            min % 60 == 0L -> "${min / 60} h"
            else -> "${min / 60} h ${min % 60} min"
        }
    }

    /** `S1 · E2`, or `null` when the numbers are unknown. */
    fun episodeCode(seasonNo: Long?, episodeNo: Long?): String? =
        if (seasonNo != null && episodeNo != null) "S$seasonNo · E$episodeNo" else null

    fun playLabel(resumeS: Double, episode: String? = null, remaining: String? = null): String {
        if (resumeS <= 0) return listOfNotNull("Play", episode).joinToString(" ")
        val target = episode ?: "from ${clock(resumeS)}"
        return listOfNotNull("Resume $target", remaining).joinToString(" · ")
    }

    /** Prefer a resumable episode, then an unwatched episode, then the first episode. */
    fun nextUp(episodes: List<ItemSummary>): ItemSummary? =
        episodes.firstOrNull { !it.played && (it.progress ?: 0.0) in STARTED..<FINISHED }
            ?: episodes.firstOrNull { !it.played && (it.progress ?: 0.0) < FINISHED }
            ?: episodes.firstOrNull()

    /** The season a show opens on: the first one not fully watched, else the first. */
    fun defaultSeason(seasons: List<ItemSummary>): ItemSummary? = seasons.firstOrNull { !it.played } ?: seasons.firstOrNull()

    /** Eyebrow over the title. */
    fun kindLabel(d: ItemDetail): String = when (d.kind) {
        "show" -> "Series"
        "season" -> "Season"
        "episode" -> episodeCode(d.seasonNo, d.episodeNo) ?: "Episode"
        else -> "Film"
    }

    /** Year · rating badge · runtime · seasons · genres, as separate chips (the ★ rating is shown apart). */
    fun meta(d: ItemDetail): List<String> = buildList {
        d.year?.let { add(it.toString()) }
        d.contentRating?.takeIf { it.isNotBlank() }?.let { add(it) }
        val runtime = d.runtimeS ?: d.files.firstOrNull()?.durationS?.roundToLong()
        if (d.kind != "show" && d.kind != "season" && runtime != null && runtime > 0) add(runtime(runtime))
        if (d.kind == "show" && d.childrenCount > 0) add(if (d.childrenCount == 1L) "1 season" else "${d.childrenCount} seasons")
        if (d.kind == "season" && d.childrenCount > 0) add(if (d.childrenCount == 1L) "1 episode" else "${d.childrenCount} episodes")
        if (d.genres.isNotEmpty()) add(d.genres.take(3).joinToString(" / "))
    }

    /** `★ 7.8`, or `null` without a rating. */
    fun stars(rating: Double?): String? = rating?.takeIf { it > 0 }?.let { "★ " + String.format(Locale.ROOT, "%.1f", it) }

    /** `18 min left` for a title in progress; `null` when not started or the runtime is unknown. */
    fun timeLeft(progress: Double?, durationS: Double?): String? {
        if (progress == null || durationS == null || durationS <= 0 || progress < STARTED || progress >= FINISHED) return null
        val left = ((1 - progress) * durationS).roundToLong().coerceAtLeast(60)
        return "${runtime(left)} left"
    }

    /**
     * Badges for a file: `4K`, `HDR10` / `HLG` / `Dolby Vision`, `5.1` / `7.1`. (Whether it plays
     * directly is only known once the server decides, so there's no *Direct play* badge here.)
     */
    fun badges(f: MediaFile?): List<String> = buildList {
        val v = f?.video ?: return@buildList
        if (v.width >= 3200 || v.height >= 2000) add("4K")
        when (v.hdr) {
            "hdr10" -> add("HDR10")
            "hlg" -> add("HLG")
            "dv" -> add("Dolby Vision")
        }
        when (f.audio.maxOfOrNull { it.channels } ?: 0) {
            in 7L..7L -> add("6.1")
            in 8L..Long.MAX_VALUE -> add("7.1")
            6L -> add("5.1")
            else -> Unit
        }
    }

    /** Episode row caption: `50 min · 18 min left`, `52 min · Watched`. */
    fun episodeMeta(e: ItemSummary): String? {
        val dur = e.runtimeS?.takeIf { it > 0 }
        val state = when {
            e.played -> "Watched"
            else -> timeLeft(e.progress, dur?.toDouble())
        }
        return listOfNotNull(dur?.let { runtime(it) }, state).joinToString(" · ").ifEmpty { null }
    }

    /** Caption under an episode card: `E3 · 45 min`. */
    fun episodeCaption(e: ItemSummary): String? {
        val parts = listOfNotNull(e.episodeNo?.let { "E$it" }, e.runtimeS?.takeIf { it > 0 }?.let { runtime(it) })
        return parts.joinToString(" · ").ifEmpty { null }
    }
}

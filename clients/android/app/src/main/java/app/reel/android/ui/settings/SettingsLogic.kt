package app.reel.android.ui.settings

import java.util.Locale

/** Pure helpers for the Settings page (#194). */
object SettingsLogic {
    /** Languages offered when adding one (same list as the web's picker). */
    val PICKER_LANGUAGES = listOf(
        "en", "fr", "de", "es", "it", "pt", "nl", "sv", "da", "no", "fi", "pl", "cs", "hu", "ro", "el", "tr", "ru", "uk",
        "ar", "he", "fa", "hi", "ja", "ko", "zh", "th", "vi", "id", "ms", "bg", "hr", "sr", "sk", "sl", "is",
    )

    /** `en` → English, `original` → Original language; unknown codes stay as typed. */
    fun languageName(code: String): String {
        if (code == "original") return "Original language"
        val name = Locale.forLanguageTag(code).getDisplayLanguage(Locale.ENGLISH)
        return if (name.isNullOrBlank() || name.equals(code, ignoreCase = true)) code else name
    }

    /** Languages that can still be added to [have]; audio lists can also take *Original*. */
    fun addable(have: List<String>, audio: Boolean): List<String> =
        (if (audio) listOf("original") else emptyList<String>()).plus(PICKER_LANGUAGES).filter { it !in have }

    /** [list] with item [i] moved by [delta] places (clamped). */
    fun <T> move(list: List<T>, i: Int, delta: Int): List<T> {
        if (i !in list.indices) return list
        val to = (i + delta).coerceIn(0, list.lastIndex)
        if (to == i) return list
        return list.toMutableList().apply { add(to, removeAt(i)) }
    }

    /** Subtitle modes, in the web's order and words. */
    val SUBTITLE_MODES = listOf(
        Choice("off", "Off", "Never turn subtitles on."),
        Choice("forced_only", "Forced only", "Only the parts in another language (signs, foreign dialogue)."),
        Choice("auto", "Automatic", "On when the audio isn't in one of your languages."),
        Choice("always", "Always", "Always on, in the first of your subtitle languages a title has."),
    )

    fun subtitleMode(id: String): Choice = SUBTITLE_MODES.firstOrNull { it.id == id } ?: SUBTITLE_MODES[2]

    val QUALITY_LABELS = mapOf(
        "auto" to "Automatic",
        "original" to "Original",
        "1080p" to "1080p",
        "720p" to "720p",
        "480p" to "480p",
    )

    /** `Active now`, `5 min ago`, `3 h ago`, `2 days ago`. */
    fun lastSeen(epochS: Long, nowS: Long): String {
        val d = (nowS - epochS).coerceAtLeast(0)
        return when {
            d < 120 -> "Active now"
            d < 3600 -> "${d / 60} min ago"
            d < 86_400 -> "${d / 3600} h ago"
            d < 2 * 86_400 -> "Yesterday"
            else -> "${d / 86_400} days ago"
        }
    }

    /** `Android`, `Web`, `webOS`, `Tizen` for a session's `client`. */
    fun clientName(client: String?): String? = when (client?.lowercase()) {
        null, "" -> null
        "android" -> "Android"
        "web" -> "Web"
        "webos" -> "webOS"
        "tizen" -> "Tizen"
        else -> client.replaceFirstChar { it.uppercase() }
    }
}

/** One option with a short explanation. */
data class Choice(val id: String, val label: String, val body: String)

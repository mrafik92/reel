package app.reel.android.ui.live

import app.reel.android.data.IptvChannel
import app.reel.android.data.IptvGroup
import app.reel.android.data.NowNext
import app.reel.android.data.Program
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/** A chip above the channel list. */
sealed interface ChannelFilter {
    val label: String

    data object Favourites : ChannelFilter {
        override val label = "Favourites"
    }

    data object All : ChannelFilter {
        override val label = "All"
    }

    data class Group(val name: String) : ChannelFilter {
        override val label get() = name
    }
}

/** Pure Live TV rules (#191): chips, filtering, zapping, number entry, now/next. Unit-tested. */
object LiveLogic {
    /** Favourites first (the owner's design), then All, then the provider's groups in its order. */
    fun chips(groups: List<IptvGroup>): List<ChannelFilter> =
        listOf(ChannelFilter.Favourites, ChannelFilter.All) +
            groups.filter { it.name.isNotBlank() && it.count > 0 }.map { ChannelFilter.Group(it.name) }

    /** Favourites when there are any, else everything. */
    fun defaultFilter(channels: List<IptvChannel>): ChannelFilter =
        if (channels.any { it.favorite }) ChannelFilter.Favourites else ChannelFilter.All

    fun filter(channels: List<IptvChannel>, f: ChannelFilter): List<IptvChannel> = when (f) {
        ChannelFilter.Favourites -> channels.filter { it.favorite }
        ChannelFilter.All -> channels
        is ChannelFilter.Group -> channels.filter { it.group == f.name }
    }

    /**
     * The channel [delta] steps from [currentId] in [list] (the list being shown), wrapping at
     * the ends like a TV remote. A current channel that isn't in the list zaps from its start.
     */
    fun zapTarget(list: List<IptvChannel>, currentId: Long?, delta: Int): IptvChannel? {
        if (list.isEmpty()) return null
        val i = list.indexOfFirst { it.id == currentId }
        if (i < 0) return if (delta >= 0) list.first() else list.last()
        val n = list.size
        return list[((i + delta) % n + n) % n]
    }

    /** The channel numbered [number], searched in [list] first and then in [all]. */
    fun byNumber(list: List<IptvChannel>, all: List<IptvChannel>, number: Long): IptvChannel? =
        list.firstOrNull { it.number == number } ?: all.firstOrNull { it.number == number }

    /** Where to start: the last channel watched, else the first one of the list. */
    fun startChannel(list: List<IptvChannel>, all: List<IptvChannel>, last: Long?): IptvChannel? =
        all.firstOrNull { it.id == last } ?: list.firstOrNull() ?: all.firstOrNull()

    /** How far through [p] we are at [nowS], 0..1. */
    fun progress(p: Program?, nowS: Long): Float {
        if (p == null || p.stop <= p.start) return 0f
        return ((nowS - p.start).toFloat() / (p.stop - p.start)).coerceIn(0f, 1f)
    }

    /**
     * Advance the cached guide at programme boundaries while its refresh is in flight.
     */
    fun current(nn: NowNext?, nowS: Long): Pair<Program?, Program?> {
        if (nn == null) return null to null
        val now = nn.now?.takeIf { nowS in it.start until it.stop }
        val next = nn.next?.takeIf { it.stop > nowS }
        return if (now == null && next != null && next.start <= nowS) next to null else now to next
    }

    /** "ARN1" from "Arena One", "CINE" from "Cine HD": the tile when a channel has no logo. */
    fun abbreviation(name: String): String {
        val words = name.split(Regex("[^\\p{L}\\p{N}]+"))
            .filter { it.isNotEmpty() && it.uppercase() !in QUALITY_TAGS }
        if (words.isEmpty()) return name.take(4).uppercase()
        if (words.size == 1) return words[0].take(4).uppercase()
        val last = words.last().uppercase()
        val tail = (NUMBER_WORDS[last] ?: last.takeIf { it.all(Char::isDigit) })?.take(2)
        val up = words.map { it.uppercase() }
        return when {
            // "Arena One" → ARE1, "Sport 24" → SP24
            tail != null -> up.dropLast(1).joinToString("").take(4 - tail.length) + tail
            // "BBC World News" → BWN
            up.size >= 3 -> up.take(4).joinToString("") { it.take(1) }
            // "Sky Sports" → SKYS
            else -> up[0].take(3) + up[1].take(1)
        }
    }

    private val NUMBER_WORDS = mapOf("ONE" to "1", "TWO" to "2", "THREE" to "3", "FOUR" to "4", "FIVE" to "5")

    private val QUALITY_TAGS = setOf("HD", "FHD", "UHD", "SD", "4K", "HEVC", "H265")

    /** "21:00" in the device's time zone. */
    fun clock(unixS: Long, zone: TimeZone = TimeZone.getDefault()): String =
        SimpleDateFormat("HH:mm", Locale.getDefault()).apply { timeZone = zone }.format(Date(unixS * 1000))

    /** The number a channel shows: its number, or its position in the full list. */
    fun displayNumber(c: IptvChannel, all: List<IptvChannel>): String =
        c.number?.toString() ?: (all.indexOfFirst { it.id == c.id } + 1).takeIf { it > 0 }?.toString().orEmpty()
}

/**
 * Typing a channel number on a remote: digits collect for [TIMEOUT_MS] after the last one (or until
 * [maxDigits]), then [commit] gives the number.
 */
class NumberEntry(private val maxDigits: Int = 4) {
    var digits: String = ""
        private set

    /** Add a digit. True when the entry is complete (max length reached). */
    fun type(d: Int): Boolean {
        if (d !in 0..9) return false
        if (digits.length >= maxDigits) digits = ""
        digits += d
        return digits.length >= maxDigits
    }

    /** The number typed (and clear it), or `null` if nothing was. */
    fun commit(): Long? {
        val n = digits.toLongOrNull()
        digits = ""
        return n
    }

    fun clear() {
        digits = ""
    }

    companion object {
        const val TIMEOUT_MS = 1_500L
    }
}

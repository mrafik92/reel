package app.reel.android.ui.live

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.reel.android.data.IptvChannel
import app.reel.android.data.NowNext
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.channelGroups
import app.reel.android.data.channels
import app.reel.android.data.nowNext
import app.reel.android.data.setFavorite
import app.reel.android.ui.common.Load
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What the Live TV tab shows. */
data class LiveUi(
    val channels: Load<List<IptvChannel>> = Load.Loading,
    val chips: List<ChannelFilter> = listOf(ChannelFilter.Favourites, ChannelFilter.All),
    val filter: ChannelFilter = ChannelFilter.All,
    /** Now / next by channel id. */
    val now: Map<Long, NowNext> = emptyMap(),
    /** The clock now/next was last drawn against (unix seconds). */
    val nowS: Long = 0,
    /** A favourite change failed (shown briefly). */
    val notice: String? = null,
) {
    val all: List<IptvChannel> get() = (channels as? Load.Ready)?.value.orEmpty()

    /** The channels of the selected chip, in channel order: the list, and what zapping walks. */
    val visible: List<IptvChannel> get() = LiveLogic.filter(all, filter)
}

/**
 * The Live TV tab's data (#191): channels, groups, favourites and now/next. Lives as long as the
 * tab's back-stack entry. [watchGuide] refreshes the guide for channels shown on screen. The player and its session live in [app.reel.android.player.LiveController].
 */
class LiveViewModel(
    private val api: ReelApi,
    /** Wall clock in unix seconds. */
    private val clock: () -> Long = { System.currentTimeMillis() / 1000 },
) : ViewModel() {
    private val _ui = MutableStateFlow(LiveUi())
    val ui: StateFlow<LiveUi> = _ui.asStateFlow()

    private var filterPicked = false
    private val savingFavorites = mutableSetOf<Long>()

    init {
        load()
    }

    fun load() {
        _ui.update { it.copy(channels = Load.Loading, nowS = clock()) }
        viewModelScope.launch {
            try {
                val chans = api.channels()
                _ui.update {
                    it.copy(
                        channels = Load.Ready(chans),
                        filter = if (filterPicked) it.filter else LiveLogic.defaultFilter(chans),
                    )
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _ui.update { it.copy(channels = Load.Failed(e)) }
                return@launch
            }
            // Groups and now/next are extras: the list works without them.
            val groups = try {
                api.channelGroups()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                emptyList()
            }
            _ui.update { it.copy(chips = LiveLogic.chips(groups)) }
        }
    }

    private suspend fun refreshNow(ids: List<Long>) {
        try {
            val nn = api.nowNext(ids)
            _ui.update { it.copy(now = it.now + nn.associateBy { n -> n.channelId }, nowS = clock()) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            _ui.update { it.copy(nowS = clock()) }
        }
    }

    /** Fetch only the visible and selected channels, once scrolling settles. */
    suspend fun watchGuide(ids: List<Long>) {
        if (ids.isEmpty()) return
        delay(350)
        refreshNow(ids)
        var fetched = clock()
        while (true) {
            delay(30_000)
            val ui = _ui.value
            val t = clock()
            val ended = ids.any { id -> ui.now[id]?.now?.stop?.let { it in (ui.nowS + 1)..t } == true }
            if (ended || t - fetched >= 300) {
                refreshNow(ids)
                fetched = t
            } else {
                _ui.update { it.copy(nowS = t) }
            }
        }
    }

    fun select(f: ChannelFilter) {
        filterPicked = true
        _ui.update { it.copy(filter = f) }
    }

    /** Star / unstar: shown at once, put back if the server refuses. */
    fun toggleFavorite(id: Long) {
        val before = _ui.value.all.firstOrNull { it.id == id } ?: return
        if (!savingFavorites.add(id)) return
        val on = !before.favorite
        setFav(id, on)
        viewModelScope.launch {
            try {
                api.setFavorite(id, on)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                setFav(id, !on)
                _ui.update { it.copy(notice = (e as? ReelError)?.userMessage ?: "Couldn't change your favourites") }
            } finally {
                savingFavorites.remove(id)
            }
        }
    }

    fun clearNotice() = _ui.update { it.copy(notice = null) }

    private fun setFav(id: Long, on: Boolean) {
        _ui.update { u ->
            val list = u.all.map { if (it.id == id) it.copy(favorite = on) else it }
            u.copy(channels = Load.Ready(list))
        }
    }
}

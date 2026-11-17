package app.reel.android.ui.library

import app.reel.android.data.ItemPage
import app.reel.android.data.ItemSummary
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What a keyset-paged list has loaded so far. */
data class Paged(
    val items: List<ItemSummary> = emptyList(),
    val loading: Boolean = false,
    /** The last page failed; [ItemPager.retry] tries it again. */
    val error: Throwable? = null,
    /** The server sent no `next_cursor`: everything is loaded. */
    val end: Boolean = false,
) {
    /** Nothing loaded yet and nothing on the way (first page failed, or not started). */
    val initial get() = items.isEmpty() && !end
}

/**
 * Keyset paging over `/libraries/{id}/items` (or any `ItemPage` endpoint).
 *
 * - [loadMore] fetches the next page unless one is loading, the list ended, or the last page
 *   failed (then only [retry] continues, so a dead server isn't hammered while the user scrolls).
 * - [reset] starts over (a new sort or filter) and drops any page still on its way.
 * - Items already shown are never duplicated, even if the server repeats one across pages.
 */
class ItemPager(
    private val scope: CoroutineScope,
    private val fetch: suspend (cursor: String?) -> ItemPage,
) {
    private val _state = MutableStateFlow(Paged())
    val state: StateFlow<Paged> = _state.asStateFlow()

    private var cursor: String? = null
    private var job: Job? = null

    /** Load the next page. Safe to call on every scroll frame. */
    fun loadMore() {
        val s = _state.value
        if (s.loading || s.end || s.error != null) return
        start()
    }

    fun retry() {
        if (_state.value.loading || _state.value.end) return
        _state.update { it.copy(error = null) }
        start()
    }

    /** Forget everything and load the first page again. */
    fun reset() {
        job?.cancel()
        cursor = null
        _state.value = Paged()
        start()
    }

    private fun start() {
        val from = cursor
        _state.update { it.copy(loading = true, error = null) }
        job = scope.launch {
            try {
                val page = fetch(from)
                cursor = page.nextCursor
                _state.update { s ->
                    val seen = s.items.mapTo(HashSet()) { it.id }
                    s.copy(
                        items = s.items + page.items.filter { seen.add(it.id) },
                        loading = false,
                        end = page.nextCursor.isNullOrEmpty(),
                    )
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _state.update { it.copy(loading = false, error = e) }
            }
        }
    }

    companion object {
        /**
         * True when the grid's last visible item is within [threshold] items of the end, so the
         * next page should load (focus moving down on TV scrolls the grid the same way touch does).
         */
        fun nearEnd(lastVisible: Int, total: Int, threshold: Int): Boolean = total == 0 || lastVisible >= total - 1 - threshold
    }
}

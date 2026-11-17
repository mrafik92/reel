package app.reel.android.ui.search

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.reel.android.data.SearchAll
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/** What the search page shows under the box. */
sealed interface SearchState {
    /** Fewer than [SearchViewModel.MIN_CHARS] characters typed. */
    data object Idle : SearchState

    data class Loading(val q: String) : SearchState

    data class Results(val q: String, val results: SearchAll) : SearchState

    data class Failed(val q: String, val error: Throwable) : SearchState
}

/**
 * Search (#189): the typed text, searched [DEBOUNCE_MS] after the last keystroke; only the latest
 * text's answer is shown (an older, slower answer is dropped). Lives as long as the Search tab's
 * back-stack entry, so Back from a result shows the same results.
 */
@OptIn(FlowPreview::class, ExperimentalCoroutinesApi::class)
class SearchViewModel(private val search: suspend (String) -> SearchAll) : ViewModel() {
    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query.asStateFlow()

    private val _state = MutableStateFlow<SearchState>(SearchState.Idle)
    val state: StateFlow<SearchState> = _state.asStateFlow()

    init {
        viewModelScope.launch {
            _query.map { it.trim() }
                .debounce { if (it.length < MIN_CHARS) 0 else DEBOUNCE_MS }
                .distinctUntilChanged()
                .collectLatest { q -> run(q) }
        }
    }

    fun setQuery(q: String) {
        _query.value = q
    }

    /**
     * Search now (the keyboard's search key, a voice result). [force] searches again even when
     * these results are already shown or on their way (*Try again*).
     */
    fun submit(force: Boolean = false) {
        viewModelScope.launch { run(_query.value.trim(), force) }
    }

    private suspend fun run(q: String, force: Boolean = false) {
        if (q.length < MIN_CHARS) {
            _state.value = SearchState.Idle
            return
        }
        val now = _state.value
        if (!force && ((now is SearchState.Results && now.q == q) || (now is SearchState.Loading && now.q == q))) return
        _state.value = SearchState.Loading(q)
        _state.value = try {
            SearchState.Results(q, search(q))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            SearchState.Failed(q, e)
        }.takeIf { _query.value.trim() == q } ?: return
    }

    companion object {
        const val MIN_CHARS = 2
        const val DEBOUNCE_MS = 300L
    }
}

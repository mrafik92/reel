package app.reel.android.ui.library

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.reel.android.data.GenreCount
import app.reel.android.data.Library
import app.reel.android.data.LibraryQuery
import app.reel.android.data.ReelApi
import app.reel.android.data.genres
import app.reel.android.data.libraries
import app.reel.android.data.libraryItems
import app.reel.android.ui.common.Load
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * The Library tab: which library is open, its sort / filters, the paged grid and the genre list.
 * Lives as long as the tab's back-stack entry, so going into an item and back keeps the loaded
 * pages and the scroll position.
 *
 * [initialLibrary] preselects a library (Home's *See all*); otherwise the first one opens.
 */
class LibraryViewModel(
    private val api: ReelApi,
    private val initialLibrary: Long? = null,
    private val pageSize: Int = 60,
) : ViewModel() {
    private val _libraries = MutableStateFlow<Load<List<Library>>>(Load.Loading)
    val libraries: StateFlow<Load<List<Library>>> = _libraries.asStateFlow()

    private val _selected = MutableStateFlow<Long?>(null)
    val selected: StateFlow<Long?> = _selected.asStateFlow()

    private val _query = MutableStateFlow(LibraryQuery())
    val query: StateFlow<LibraryQuery> = _query.asStateFlow()

    private val _genres = MutableStateFlow<List<GenreCount>>(emptyList())
    val genres: StateFlow<List<GenreCount>> = _genres.asStateFlow()

    val pager = ItemPager(viewModelScope) { cursor ->
        val id = _selected.value ?: error("no library selected")
        api.libraryItems(id, _query.value, cursor, pageSize)
    }

    private var genreJob: Job? = null

    init {
        loadLibraries()
    }

    fun loadLibraries() {
        _libraries.value = Load.Loading
        viewModelScope.launch {
            try {
                val libs = api.libraries()
                _libraries.value = Load.Ready(libs)
                val pick = libs.firstOrNull { it.id == initialLibrary } ?: libs.firstOrNull()
                if (pick != null) select(pick.id)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _libraries.value = Load.Failed(e)
            }
        }
    }

    /** Open another library; its genres differ, so the genre filter is cleared. */
    fun select(libraryId: Long) {
        if (_selected.value == libraryId && !pager.state.value.initial) return
        _selected.value = libraryId
        _query.value = _query.value.copy(genre = null)
        pager.reset()
        genreJob?.cancel()
        _genres.value = emptyList()
        genreJob = viewModelScope.launch {
            // The genre list is a nicety: if it fails, the filter just has nothing to offer.
            _genres.value = try {
                api.genres(libraryId).sortedBy { it.name.lowercase() }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                emptyList()
            }
        }
    }

    fun setQuery(q: LibraryQuery) {
        if (q == _query.value) return
        _query.value = q
        if (_selected.value != null) pager.reset()
    }
}

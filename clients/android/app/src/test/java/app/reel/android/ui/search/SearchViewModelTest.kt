package app.reel.android.ui.search

import app.reel.android.data.ItemSummary
import app.reel.android.data.SearchAll
import java.io.IOException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SearchViewModelTest {
    private val main = StandardTestDispatcher()

    @Before
    fun setUp() = Dispatchers.setMain(main)

    @After
    fun tearDown() = Dispatchers.resetMain()

    private fun hit(q: String) = SearchAll(library = listOf(ItemSummary(1, "movie", q)))

    private fun test(block: suspend TestScope.() -> Unit) = runTest(main) { block() }

    @Test
    fun searchesOnceTypingPauses() = test {
        val asked = mutableListOf<String>()
        val vm = SearchViewModel { q -> asked += q; hit(q) }
        vm.setQuery("d")
        vm.setQuery("du")
        vm.setQuery("dun")
        advanceTimeBy(100)
        runCurrent()
        assertTrue(asked.isEmpty())
        advanceUntilIdle()
        assertEquals(listOf("dun"), asked)
        assertEquals(SearchState.Results("dun", hit("dun")), vm.state.value)
    }

    @Test
    fun tooShortIsIdle() = test {
        val vm = SearchViewModel { q -> hit(q) }
        vm.setQuery("dune")
        advanceUntilIdle()
        vm.setQuery(" d ")
        advanceUntilIdle()
        assertEquals(SearchState.Idle, vm.state.value)
    }

    @Test
    fun onlyTheLatestAnswerIsShown() = test {
        val slow = CompletableDeferred<SearchAll>()
        val vm = SearchViewModel { q -> if (q == "old") slow.await() else hit(q) }
        vm.setQuery("old")
        advanceTimeBy(400)
        runCurrent()
        vm.setQuery("new")
        advanceUntilIdle()
        slow.complete(hit("old"))
        advanceUntilIdle()
        assertEquals(SearchState.Results("new", hit("new")), vm.state.value)
    }

    @Test
    fun submitDoesNotRepeatAndRetryForces() = test {
        var calls = 0
        var fail = true
        val vm = SearchViewModel { q ->
            calls++
            if (fail) throw IOException("down") else hit(q)
        }
        vm.setQuery("arrival")
        vm.submit() // the keyboard's search key, before the debounce fires
        advanceUntilIdle()
        assertTrue(vm.state.value is SearchState.Failed)
        val afterFirst = calls
        fail = false
        vm.submit(force = true)
        advanceUntilIdle()
        assertEquals(SearchState.Results("arrival", hit("arrival")), vm.state.value)
        vm.submit()
        advanceUntilIdle()
        assertEquals(afterFirst + 1, calls)
    }
}

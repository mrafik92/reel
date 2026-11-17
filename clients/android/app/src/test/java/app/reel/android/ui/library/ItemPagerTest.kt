package app.reel.android.ui.library

import app.reel.android.data.ItemPage
import app.reel.android.data.ItemSummary
import java.io.IOException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ItemPagerTest {
    private fun item(id: Long) = ItemSummary(id, "movie", "M$id")

    private fun page(ids: LongRange, next: String?) = ItemPage(ids.map(::item), next)

    private fun TestScope.pager(fetch: suspend (String?) -> ItemPage) =
        ItemPager(TestScope(UnconfinedTestDispatcher(testScheduler)), fetch)

    @Test
    fun pagesFollowTheCursorUntilTheEnd() = runTest {
        val asked = mutableListOf<String?>()
        val p = pager { c ->
            asked += c
            when (c) {
                null -> page(1L..3L, "a")
                "a" -> page(4L..5L, "b")
                else -> page(6L..6L, null)
            }
        }
        p.reset()
        assertEquals(3, p.state.value.items.size)
        p.loadMore()
        p.loadMore()
        assertTrue(p.state.value.end)
        p.loadMore() // ended: no more requests
        assertEquals(listOf(null, "a", "b"), asked)
        assertEquals((1L..6L).toList(), p.state.value.items.map { it.id })
    }

    @Test
    fun loadMoreWhileLoadingIsIgnored() = runTest {
        val gate = CompletableDeferred<ItemPage>()
        var calls = 0
        val p = pager {
            calls++
            gate.await()
        }
        p.reset()
        p.loadMore()
        p.loadMore()
        assertTrue(p.state.value.loading)
        gate.complete(page(1L..2L, null))
        assertEquals(1, calls)
        assertFalse(p.state.value.loading)
    }

    @Test
    fun aFailureStopsPagingUntilRetry() = runTest {
        var fail = true
        var calls = 0
        val p = pager { c ->
            calls++
            if (c == "a" && fail) throw IOException("down")
            if (c == null) page(1L..2L, "a") else page(3L..3L, null)
        }
        p.reset()
        p.loadMore()
        assertNotNull(p.state.value.error)
        p.loadMore() // scrolling doesn't hammer a failing server
        assertEquals(2, calls)
        fail = false
        p.retry()
        assertNull(p.state.value.error)
        assertEquals(3, p.state.value.items.size)
        assertTrue(p.state.value.end)
    }

    @Test
    fun resetDropsAPageStillOnItsWay() = runTest {
        val slow = CompletableDeferred<ItemPage>()
        var first = true
        val p = pager {
            if (first) {
                first = false
                slow.await()
            } else {
                page(10L..11L, null)
            }
        }
        p.reset()
        p.reset() // a new filter while the first page is loading
        slow.complete(page(1L..2L, "x"))
        assertEquals(listOf(10L, 11L), p.state.value.items.map { it.id })
    }

    @Test
    fun repeatedItemsAcrossPagesAreShownOnce() = runTest {
        val p = pager { c -> if (c == null) page(1L..3L, "a") else page(3L..4L, null) }
        p.reset()
        p.loadMore()
        assertEquals(listOf(1L, 2L, 3L, 4L), p.state.value.items.map { it.id })
    }

    @Test
    fun nearEnd() {
        assertTrue(ItemPager.nearEnd(lastVisible = -1, total = 0, threshold = 5))
        assertFalse(ItemPager.nearEnd(lastVisible = 10, total = 60, threshold = 12))
        assertTrue(ItemPager.nearEnd(lastVisible = 47, total = 60, threshold = 12))
        assertTrue(ItemPager.nearEnd(lastVisible = 59, total = 60, threshold = 12))
    }
}

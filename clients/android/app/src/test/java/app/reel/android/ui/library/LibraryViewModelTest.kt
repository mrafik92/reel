package app.reel.android.ui.library

import app.reel.android.data.LibrarySort
import app.reel.android.data.ReelApi
import app.reel.android.data.ServerAddress
import app.reel.android.ui.common.Load
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** The Library tab against a fake server: library pick, query params, paging, filters. */
@OptIn(ExperimentalCoroutinesApi::class)
class LibraryViewModelTest {
    private lateinit var server: MockWebServer
    private val itemQueries = mutableListOf<String>()

    private fun json(body: String) = MockResponse().setHeader("Content-Type", "application/json").setBody(body)

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        server = MockWebServer()
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse {
                val url = request.requestUrl!!
                return when {
                    url.encodedPath == "/api/v1/libraries" ->
                        json("""[{"id":1,"name":"Movies","kind":"movies","item_count":3},{"id":2,"name":"Shows","kind":"shows","item_count":1}]""")
                    url.encodedPath == "/api/v1/genres" -> json("""[{"name":"Drama","count":2},{"name":"action","count":1}]""")
                    url.encodedPath.matches(Regex("/api/v1/libraries/\\d+/items")) -> {
                        synchronized(itemQueries) { itemQueries += url.encodedPath + "?" + (url.encodedQuery ?: "") }
                        if (url.queryParameter("cursor") == null) {
                            json("""{"items":[{"id":10,"kind":"movie","title":"A","played":false,"added_at":1}],"next_cursor":"c1"}""")
                        } else {
                            json("""{"items":[{"id":11,"kind":"movie","title":"B","played":true,"added_at":1}],"next_cursor":null}""")
                        }
                    }
                    else -> MockResponse().setResponseCode(404)
                }
            }
        }
        server.start()
    }

    @After
    fun tearDown() {
        server.shutdown()
        Dispatchers.resetMain()
    }

    private fun api() = ReelApi(
        ServerAddress.parse(server.url("/").toString())!!,
        OkHttpClient.Builder().connectTimeout(2, TimeUnit.SECONDS).build(),
        token = { "t" },
    )

    private fun <T> await(flow: kotlinx.coroutines.flow.StateFlow<T>, ok: (T) -> Boolean): T =
        runBlocking { withTimeout(5_000) { flow.first(ok) } }

    @Test
    fun opensTheFirstLibraryAndPagesThroughIt() {
        val vm = LibraryViewModel(api(), pageSize = 1)
        await(vm.libraries) { it is Load.Ready }
        assertEquals(1L, vm.selected.value)
        await(vm.pager.state) { it.items.size == 1 && !it.loading }
        vm.pager.loadMore()
        val s = await(vm.pager.state) { it.end }
        assertEquals(listOf(10L, 11L), s.items.map { it.id })
        assertEquals(listOf("action", "Drama"), await(vm.genres) { it.isNotEmpty() }.map { it.name })
        assertEquals("/api/v1/libraries/1/items?sort=title&limit=1", itemQueries[0])
        assertEquals("/api/v1/libraries/1/items?sort=title&cursor=c1&limit=1", itemQueries[1])
    }

    @Test
    fun aPreselectedLibraryOpensFirst() {
        val vm = LibraryViewModel(api(), initialLibrary = 2)
        await(vm.pager.state) { it.items.isNotEmpty() }
        assertEquals(2L, vm.selected.value)
    }

    @Test
    fun filtersRestartPagingWithTheirParameters() {
        val vm = LibraryViewModel(api(), pageSize = 5)
        await(vm.pager.state) { it.items.isNotEmpty() }
        vm.setQuery(vm.query.value.copy(sort = LibrarySort.Added, genre = "Drama", unwatched = true))
        await(vm.pager.state) { it.items.isNotEmpty() && !it.loading }
        assertEquals("/api/v1/libraries/1/items?sort=added&genre=Drama&unwatched=true&limit=5", itemQueries.last())
        assertEquals(1, vm.pager.state.value.items.size)
    }

    @Test
    fun switchingLibraryClearsTheGenre() {
        val vm = LibraryViewModel(api())
        await(vm.pager.state) { it.items.isNotEmpty() }
        vm.setQuery(vm.query.value.copy(genre = "Drama"))
        vm.select(2)
        assertNull(vm.query.value.genre)
        await(vm.pager.state) { it.items.isNotEmpty() && !it.loading }
        assertTrue(itemQueries.last().startsWith("/api/v1/libraries/2/items?sort=title&limit="))
    }
}

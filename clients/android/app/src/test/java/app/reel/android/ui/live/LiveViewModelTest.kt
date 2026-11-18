package app.reel.android.ui.live

import androidx.lifecycle.viewModelScope
import app.reel.android.data.IptvChannel
import app.reel.android.data.Me
import app.reel.android.data.NowNext
import app.reel.android.data.OwnLiveSession
import app.reel.android.data.Program
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.ServerAddress
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LiveViewModelTest {
    private lateinit var server: MockWebServer
    private lateinit var api: ReelApi
    private var viewModel: LiveViewModel? = null

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        server = MockWebServer().apply { start() }
        api = ReelApi(ServerAddress.parse(server.url("/").toString())!!, OkHttpClient(), token = { "test" })
    }

    @After
    fun tearDown() {
        try {
            // Keep the test Main dispatcher until in-flight ReelApi continuations have finished.
            runBlocking { viewModel?.viewModelScope?.coroutineContext?.get(Job)?.cancelAndJoin() }
            server.shutdown()
        } finally {
            Dispatchers.resetMain()
        }
    }

    private fun createViewModel(clock: () -> Long = { 90 }) =
        LiveViewModel(api, clock).also { viewModel = it }

    private fun respond(body: String, status: Int = 200) {
        server.enqueue(MockResponse().setResponseCode(status).setHeader("Content-Type", "application/json").setBody(body))
    }

    private fun request() = checkNotNull(server.takeRequest(5, TimeUnit.SECONDS))

    @Test
    fun loadsGroupsAndFavouritesAndFetchesGuideForTheVisibleChannels() = runBlocking {
        respond("""[{"id":1,"name":"News 24","group":"News","favorite":true},{"id":501,"name":"Arena One","group":"Sport"}]""")
        respond("""[{"name":"News","count":1},{"name":"Sport","count":1}]""")
        respond("""[{"channel_id":501,"now":{"start":60,"stop":120,"title":"Match"},"next":{"start":120,"stop":180,"title":"Review"}}]""")
        val vm = createViewModel()
        val loaded = withTimeout(5_000) { vm.ui.first { it.chips.size == 4 } }
        assertEquals(listOf("Favourites", "All", "News", "Sport"), loaded.chips.map { it.label })
        assertEquals(listOf(1L), loaded.visible.map { it.id })
        vm.select(ChannelFilter.Group("Sport"))
        assertEquals(listOf(501L), vm.ui.value.visible.map { it.id })

        val guide = launch { vm.watchGuide(listOf(501L)) }
        val ready = withTimeout(5_000) { vm.ui.first { it.now.isNotEmpty() } }
        guide.cancelAndJoin()
        assertEquals("Match", ready.now[501]?.now?.title)
        assertEquals("Review", ready.now[501]?.next?.title)
        assertEquals("/api/v1/iptv/channels", request().path)
        assertEquals("/api/v1/iptv/groups", request().path)
        assertEquals("/api/v1/iptv/now?channels=501", request().path)
    }

    @Test
    fun aRejectedFavouriteWriteRestoresThePreviousValue() = runBlocking {
        respond("""[{"id":1,"name":"News 24"}]""")
        respond("""[{"name":"News","count":1}]""")
        respond("""{"error":"Favourite unavailable"}""", 403)
        val vm = createViewModel()
        withTimeout(5_000) { vm.ui.first { it.chips.size == 3 } }
        vm.toggleFavorite(1)
        val failed = withTimeout(5_000) { vm.ui.first { it.notice != null } }
        assertFalse(failed.all.single().favorite)
        request()
        request()
        val save = request()
        assertEquals("POST", save.method)
        assertEquals("/api/v1/iptv/channels/1/favorite", save.path)
    }

    @Test
    fun aBusyAnswerCanStopTheRememberedSessionAndRetry() = runBlocking {
        val me = Me(1, "viewer", "user")
        val previous = OwnLiveSession("previous", "News 24")
        val saved = mutableListOf<OwnLiveSession?>()
        val source = ApiLiveSource(api, me, previous, saved::add)
        respond("""{"error":"All connections are in use"}""", 429)
        respond("", 204)
        respond("""{"session_id":"new","mode":"passthrough","url":"/stream.ts?sig=test","channel_id":3,"channel_name":"Arena One"}""")
        val busy = runCatching { source.tune(3, false) }.exceptionOrNull() as ReelError
        val action = checkNotNull(source.errorFor(busy) {}.action)
        assertEquals("Stop the other stream", action.label)
        action.run()
        assertEquals("new", source.tune(3, false).sessionId)
        assertTrue(request().body.readUtf8().contains(""""preset":"android_exoplayer""""))
        val stop = request()
        assertEquals("DELETE", stop.method)
        assertEquals("/api/v1/iptv/sessions/previous", stop.path)
        assertEquals("/api/v1/iptv/channels/3/tune", request().path)
        assertEquals(listOf(null), saved)
    }

    @Test
    fun zappingWrapsAndDigitsPreferTheCurrentGroup() {
        val a = IptvChannel(1, "News", 101)
        val b = IptvChannel(2, "Sport", 101)
        assertEquals(a, LiveLogic.zapTarget(listOf(a, b), b.id, 1))
        assertEquals(b, LiveLogic.zapTarget(listOf(a, b), a.id, -1))
        assertNull(LiveLogic.zapTarget(emptyList(), a.id, 1))
        val number = NumberEntry()
        assertFalse(number.type(1))
        assertFalse(number.type(0))
        assertFalse(number.type(1))
        assertEquals(b, LiveLogic.byNumber(listOf(b), listOf(a, b), number.commit()!!))
        assertNull(number.commit())
    }

    @Test
    fun aGapInTheGuideKeepsTheFutureProgrammeInNext() {
        val now = Program(0, 60, "News")
        val next = Program(90, 120, "Sport")
        val guide = NowNext(1, now, next)
        assertEquals(null to next, LiveLogic.current(guide, 70))
        assertEquals(next to null, LiveLogic.current(guide, 90))
    }
}

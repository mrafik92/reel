package app.reel.android.ui.vod

import app.reel.android.data.Me
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.ServerAddress
import app.reel.android.player.OpenRequest
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
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

class VodSourceTest {
    private lateinit var server: MockWebServer
    private lateinit var api: ReelApi
    private val me = Me(id = 7, username = "ana", role = "user")
    private val admin = Me(id = 1, username = "root", role = "admin")

    @Before
    fun setUp() {
        server = MockWebServer().apply { start() }
        api = ReelApi(ServerAddress.parse(server.url("/").toString())!!, OkHttpClient(), token = { "tok" }, retryDelayMs = 1)
    }

    @After
    fun tearDown() = server.shutdown()

    private val answer = """{"session_id":"v1","mode":"direct_play","url":"/api/v1/vod/direct/v1?sig=x","decision":{},
        "subtitles":[],"audio":[],"selected":{"reason":"r"},"bitrate_kbps":0,"quality":"original","quality_options":[],"variants":[]}"""

    private fun ok() = MockResponse().setBody(answer).setHeader("Content-Type", "application/json")

    @Test
    fun playSendsKindIdPresetAndAFlooredStart() = runBlocking {
        server.enqueue(ok())
        val d = VodSource(api, "episode", 991, "dev", me).open(OpenRequest(startS = 61.7, subtitleIndex = -1))
        assertEquals("v1", d.sessionId)
        val r = server.takeRequest()
        assertEquals("/api/v1/vod/play", r.path)
        val b = Json.parseToJsonElement(r.body.readUtf8()).jsonObject
        assertEquals("episode", b["kind"]!!.jsonPrimitive.content)
        assertEquals("991", b["id"]!!.jsonPrimitive.content)
        assertEquals("android_exoplayer", b["preset"]!!.jsonPrimitive.content)
        assertEquals("dev", b["device_id"]!!.jsonPrimitive.content)
        assertEquals("61.0", b["start_s"]!!.jsonPrimitive.content)
        assertFalse("subtitle_index" in b)
        assertFalse("takeover" in b)
    }

    @Test
    fun takeoverIsSentOnceAfterTheAdminAgrees() = runBlocking {
        server.enqueue(
            MockResponse().setResponseCode(409).setBody(
                """{"error":"busy","takeover":true,"holders":[{"user_id":3,"username":"sam","session_id":"l9","channel_name":"News 24"}]}""",
            ),
        )
        server.enqueue(ok())
        server.enqueue(ok())
        val src = VodSource(api, "movie", 5, "dev", admin)
        val err = try {
            src.open(OpenRequest(0.0))
            error("expected 409")
        } catch (e: ReelError) {
            src.errorFor(e)
        }
        assertEquals("Your IPTV connection is in use", err.title)
        assertTrue(err.body, err.body.startsWith("sam is watching the channel News 24."))
        assertEquals("Take over", err.action?.label)
        err.action!!.run()
        src.open(OpenRequest(0.0))
        src.open(OpenRequest(0.0))
        server.takeRequest()
        assertEquals("true", Json.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject["takeover"]!!.jsonPrimitive.content)
        assertFalse("takeover" in Json.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject)
    }

    @Test
    fun myOtherScreenCanBeStopped() = runBlocking {
        server.enqueue(
            MockResponse().setResponseCode(429).setHeader("Retry-After", "120").setBody(
                """{"error":"busy","holder":{"mine":true,"session_id":"s42","kind":"vod","title":"Dune","device":"Living room TV"}}""",
            ),
        )
        server.enqueue(MockResponse().setResponseCode(204))
        val src = VodSource(api, "movie", 5, "dev", me)
        val err = try {
            src.open(OpenRequest(0.0))
            error("expected 429")
        } catch (e: ReelError) {
            src.errorFor(e)
        }
        assertEquals("You're already watching IPTV on another screen", err.title)
        assertTrue(err.body, err.body.startsWith("You're watching Dune on Living room TV."))
        err.action!!.run()
        server.takeRequest()
        val stop = server.takeRequest()
        assertEquals("DELETE /api/v1/playback/s42", "${stop.method} ${stop.path}")
    }
}

class VodBusyWordingTest {
    private val noop: suspend (VodHolder) -> Unit = {}

    @Test
    fun someoneElseWithoutDetailsGetsAPlainExplanation() {
        val e = ReelError.RateLimited("All 1 connection(s) to the provider are in use by other viewers", 300, """{"error":"x"}""")
        val err = vodPlayError(e, Me(1, "a", "user"), {}, noop)
        assertEquals("Your IPTV connection is in use", err.title)
        assertTrue(err.body, err.body.startsWith("Someone else in your household is watching IPTV right now."))
        assertTrue(err.body, err.body.endsWith("Or try again in 5 min."))
        assertNull(err.action)
    }

    @Test
    fun aNamedHolderIsNamed() {
        val e = ReelError.RateLimited("busy", null, """{"error":"busy","holder":{"user_id":3,"username":"sam","kind":"vod","title":"Dune"}}""")
        val err = vodPlayError(e, Me(1, "a", "user"), {}, noop)
        assertTrue(err.body, err.body.startsWith("sam is watching Dune."))
    }

    @Test
    fun otherFailures() {
        assertFalse(vodPlayError(ReelError.Http(404, "Not found"), null, {}, noop).retry)
        assertEquals("Your IPTV provider didn't answer", vodPlayError(ReelError.Http(502, "Provider timeout"), null, {}, noop).title)
        assertEquals("Can't reach Reel", vodPlayError(ReelError.Unreachable(), null, {}, noop).title)
        // A plain 409 without a takeover offer isn't "busy".
        assertEquals("Couldn't start this title", vodPlayError(ReelError.Http(409, "Conflict", """{"error":"Conflict"}"""), null, {}, noop).title)
    }

    @Test
    fun holderShapes() {
        assertEquals(listOf(VodHolder(username = "x", kind = "live", title = "Ch", sessionId = "l1")), holdersOf("""{"holders":[{"username":"x","channel_name":"Ch","session_id":"l1"}]}"""))
        assertEquals(emptyList<VodHolder>(), holdersOf("not json"))
    }

    @Test
    fun resume() {
        assertEquals(1500.0, vodResumeS(0.25, 6000), 1e-9)
        assertEquals(0.0, vodResumeS(0.97, 6000), 0.0)
        assertEquals(0.0, vodResumeS(0.5, null), 0.0)
    }
}

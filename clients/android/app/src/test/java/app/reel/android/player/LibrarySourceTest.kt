package app.reel.android.player

import app.reel.android.data.ProgressRequest
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.data.ServerAddress
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
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** The library source's wire format against a mock server. */
class LibrarySourceTest {
    private lateinit var server: MockWebServer
    private lateinit var api: ReelApi

    @Before
    fun setUp() {
        server = MockWebServer().apply { start() }
        api = ReelApi(ServerAddress.parse(server.url("/").toString())!!, OkHttpClient(), token = { "tok" }, retryDelayMs = 1)
    }

    @After
    fun tearDown() = server.shutdown()

    private val answer = """{"session_id":"s1","mode":"direct_play","url":"/api/v1/direct/7?sig=x","decision":{"mode":"direct_play",
        "video":{"type":"copy"},"audio":{"type":"copy"},"subtitle":{"type":"native"},"reasons":["Direct play: everything fits"]},
        "subtitles":[],"audio":[],"selected":{"reason":"r"},"bitrate_kbps":5000,"quality":"original",
        "quality_options":[{"id":"original","label":"Original","bitrate_kbps":5000,"available":true}],"variants":[]}"""

    @Test
    fun decideSendsThePresetDeviceItemAndStart() = runBlocking {
        server.enqueue(MockResponse().setBody(answer).setHeader("Content-Type", "application/json"))
        val d = LibrarySource(api, fileId = 7, itemId = 42, deviceId = "dev-1").open(OpenRequest(startS = 61.5, subtitleIndex = -1))
        assertEquals("native", d.decision.subtitle.type)
        assertEquals("original", d.qualityOptions.single().id)
        assertTrue(d.isDirect)
        assertFalse(d.isHls)
        val r = server.takeRequest()
        assertEquals("/api/v1/playback/7/decide", r.path)
        assertEquals("Bearer tok", r.getHeader("Authorization"))
        val body = Json.parseToJsonElement(r.body.readUtf8()).jsonObject
        assertEquals("android_exoplayer", body["preset"]!!.jsonPrimitive.content)
        assertEquals("dev-1", body["device_id"]!!.jsonPrimitive.content)
        assertEquals("42", body["item_id"]!!.jsonPrimitive.content)
        assertEquals("61.5", body["start_s"]!!.jsonPrimitive.content)
        assertEquals("-1", body["subtitle_index"]!!.jsonPrimitive.content)
        assertEquals("auto", body["quality"]!!.jsonPrimitive.content)
        assertFalse("audio_index" in body)
    }

    @Test
    fun progressAndStop() = runBlocking {
        server.enqueue(MockResponse().setResponseCode(204))
        server.enqueue(MockResponse().setResponseCode(204))
        val src = LibrarySource(api, 7, 42, "dev-1")
        src.progress("s1", ProgressRequest(12.0, paused = true))
        src.stop("s1")
        val p = server.takeRequest()
        assertEquals("POST /api/v1/playback/s1/progress", "${p.method} ${p.path}")
        assertEquals("""{"position_s":12.0,"paused":true}""", p.body.readUtf8())
        val s = server.takeRequest()
        assertEquals("DELETE /api/v1/playback/s1", "${s.method} ${s.path}")
    }

    @Test
    fun aReapedSessionIsA404() = runBlocking {
        server.enqueue(MockResponse().setResponseCode(404).setBody("""{"error":"no such session"}"""))
        try {
            LibrarySource(api, 7, 42, null).progress("gone", ProgressRequest(5.0, false))
            fail("expected 404")
        } catch (e: ReelError.Http) {
            assertEquals(404, e.status)
        }
    }

    @Test
    fun aMissingFileSaysSo() {
        val e = LibrarySource(api, 7, 42, null).errorFor(ReelError.Http(404, "Not found"))
        assertEquals("File not found", e.title)
        assertFalse(e.retry)
    }
}

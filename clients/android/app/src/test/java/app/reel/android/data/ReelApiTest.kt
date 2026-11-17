package app.reel.android.data

import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.SocketPolicy
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class ReelApiTest {
    private lateinit var server: MockWebServer
    private var token: String? = "secret-token"
    private var online = true
    private var unauthorized = 0
    private val http = OkHttpClient.Builder().connectTimeout(2, TimeUnit.SECONDS).build()

    private fun api(readTimeoutMs: Long = 5_000) = ReelApi(
        server = ServerAddress.parse(server.url("/").toString())!!,
        base = http,
        token = { token },
        isOnline = { online },
        onUnauthorized = { unauthorized++ },
        readTimeoutMs = readTimeoutMs,
        writeTimeoutMs = readTimeoutMs,
        retryDelayMs = 10,
    )

    private inline fun <reified E : ReelError> expect(block: () -> Unit): E {
        try {
            block()
        } catch (e: ReelError) {
            if (e is E) return e
            fail("expected ${E::class.simpleName}, got $e")
        }
        fail("expected ${E::class.simpleName}, got success")
        error("unreachable")
    }

    private fun json(body: String, code: Int = 200) =
        MockResponse().setResponseCode(code).setHeader("Content-Type", "application/json").setBody(body)

    @Before
    fun setUp() {
        server = MockWebServer().apply { start() }
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    @Test
    fun loginSendsTheAndroidClientAndNoToken(): Unit = runBlocking {
        server.enqueue(
            json("""{"token":"t1","expires_at":1900000000,"user":{"id":7,"username":"rafik","role":"user","limits":{},"permissions":{"iptv_access":true,"vod_access":false,"can_request":true,"can_cast":false},"direct_live":false}}"""),
        )
        val res = api().login(LoginBody("rafik", "pw", deviceId = "dev-1", deviceName = "Google Pixel 7"))
        assertEquals("t1", res.token)
        assertEquals("rafik", res.user.username)
        assertTrue(res.user.canWatchLive)

        val req = server.takeRequest()
        assertEquals("POST", req.method)
        assertEquals("/api/v1/auth/login", req.path)
        assertNull("sign-in never carries a token", req.getHeader("Authorization"))
        val body = Json.parseToJsonElement(req.body.readUtf8()).jsonObject
        assertEquals("android", body["client"]!!.jsonPrimitive.content)
        assertEquals("dev-1", body["device_id"]!!.jsonPrimitive.content)
        assertEquals("Google Pixel 7", body["device_name"]!!.jsonPrimitive.content)
        assertEquals("pw", body["password"]!!.jsonPrimitive.content)
    }

    @Test
    fun wrongPasswordIsAnHttp401NotASessionEnd(): Unit = runBlocking {
        server.enqueue(json("""{"error":"invalid username or password"}""", 401))
        val e = expect<ReelError.Http> { runBlocking { api().login(LoginBody("a", "b")) } }
        assertEquals(401, e.status)
        assertEquals("invalid username or password", e.message)
        assertEquals("a failed sign-in doesn't sign anyone out", 0, unauthorized)
    }

    @Test
    fun signedInCallsCarryTheBearer(): Unit = runBlocking {
        server.enqueue(json("""{"id":1,"username":"u","role":"admin","limits":{},"permissions":{"iptv_access":false,"vod_access":false,"can_request":false,"can_cast":false},"direct_live":true}"""))
        val me = api().me()
        assertTrue(me.isAdmin)
        assertEquals("Bearer secret-token", server.takeRequest().getHeader("Authorization"))
    }

    @Test
    fun a401OnASignedInCallEndsTheSession(): Unit = runBlocking {
        server.enqueue(json("""{"error":"unauthorized"}""", 401))
        expect<ReelError.Unauthorized> { runBlocking { api().me() } }
        assertEquals(1, unauthorized)
    }

    @Test
    fun lockoutCarriesRetryAfter(): Unit = runBlocking {
        server.enqueue(json("""{"error":"Too many failed sign-ins."}""", 429).setHeader("Retry-After", "300"))
        val e = expect<ReelError.RateLimited> { runBlocking { api().login(LoginBody("a", "b")) } }
        assertEquals(300L, e.retryAfterS)
        assertEquals("Too many failed sign-ins. Try again in 5 min.", e.userMessage)
    }

    @Test
    fun otherErrorsKeepStatusAndBody(): Unit = runBlocking {
        server.enqueue(json("""{"error":"channel busy","takeover":true}""", 409))
        val e = expect<ReelError.Http> { runBlocking { api().post<TuneBody, TuneResponse>("/iptv/channels/3/tune", TuneBody()) } }
        assertEquals(409, e.status)
        assertEquals("channel busy", e.message)
        assertTrue(e.body!!.contains("takeover"))

        server.enqueue(MockResponse().setResponseCode(503))
        val e2 = expect<ReelError.Http> { runBlocking { api().get<Health>("/health") } }
        assertEquals("Server error (503)", e2.message)
    }

    @Test
    fun timeoutIsClassified(): Unit = runBlocking {
        repeat(2) { server.enqueue(MockResponse().setSocketPolicy(SocketPolicy.NO_RESPONSE)) }
        expect<ReelError.Timeout> { runBlocking { api(readTimeoutMs = 300).health() } }
    }

    @Test
    fun unreachableAndOffline(): Unit = runBlocking {
        val a = api()
        server.shutdown()
        expect<ReelError.Unreachable> { runBlocking { a.health() } }
        online = false
        expect<ReelError.Offline> { runBlocking { a.health() } }
    }

    @Test
    fun readsAreRetriedOnceWritesAreNot(): Unit = runBlocking {
        server.enqueue(MockResponse().setSocketPolicy(SocketPolicy.DISCONNECT_AT_START))
        server.enqueue(json("""{"status":"ok"}"""))
        assertEquals("ok", api().health().status)
        assertEquals(2, server.requestCount)

        // OkHttp itself replays a request once on a stale pooled connection; on a fresh connection a
        // write the server got isn't repeated, by OkHttp or by ReelApi.
        http.connectionPool.evictAll()
        server.enqueue(MockResponse().setSocketPolicy(SocketPolicy.DISCONNECT_AFTER_REQUEST))
        server.enqueue(json("""{}"""))
        expect<ReelError.Unreachable> { runBlocking { api().postEmpty("/auth/logout") } }
        assertEquals(3, server.requestCount)
    }

    @Test
    fun garbageIsABadResponse(): Unit = runBlocking {
        server.enqueue(MockResponse().setBody("<html>router login</html>"))
        expect<ReelError.BadResponse> { runBlocking { api().health() } }
    }

    @Test
    fun theTokenNeverLeavesTheServerOrigin(): Unit = runBlocking {
        val other = MockWebServer().apply { start() }
        try {
            val a = api()
            // A redirect to another origin (e.g. a CDN): the follow-up must not carry the token.
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", other.url("/elsewhere").toString()))
            other.enqueue(json("""{"status":"ok"}"""))
            a.health() // a NoAuth call...
            server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", other.url("/img").toString()))
            other.enqueue(json("""{"id":1,"username":"u","role":"user","limits":{},"permissions":{"iptv_access":false,"vod_access":false,"can_request":false,"can_cast":false},"direct_live":false}"""))
            a.me() // ...and a signed-in one
            assertNull(server.takeRequest().getHeader("Authorization"))
            assertEquals("Bearer secret-token", server.takeRequest().getHeader("Authorization"))
            assertNull(other.takeRequest().getHeader("Authorization"))
            assertNull(other.takeRequest().getHeader("Authorization"))

            // Any other request on the same client (images, media) goes out without it.
            other.enqueue(MockResponse().setBody("x"))
            a.client.newCall(Request.Builder().url(other.url("/poster.jpg")).build()).execute().close()
            assertNull(other.takeRequest().getHeader("Authorization"))
            // Even a header someone set by hand is stripped off-origin.
            other.enqueue(MockResponse().setBody("x"))
            a.client.newCall(Request.Builder().url(other.url("/x")).header("Authorization", "Bearer leak").build()).execute().close()
            assertNull(other.takeRequest().getHeader("Authorization"))
        } finally {
            other.shutdown()
        }
    }

    @Test
    fun queryParamsSkipNulls(): Unit = runBlocking {
        server.enqueue(json("""{"items":[],"next_cursor":null}"""))
        api().get<ItemPage>("/libraries/2/items", mapOf("cursor" to null, "limit" to 50, "sort" to ""))
        assertEquals("/api/v1/libraries/2/items?limit=50", server.takeRequest().path)
    }
}

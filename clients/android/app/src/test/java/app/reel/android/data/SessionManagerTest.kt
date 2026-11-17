package app.reel.android.data

import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class SessionManagerTest {
    private lateinit var server: MockWebServer
    private lateinit var dir: File
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val http = OkHttpClient()
    private val me = """{"id":7,"username":"rafik","role":"user","limits":{},"permissions":{"iptv_access":false,"vod_access":true,"can_request":true,"can_cast":false},"direct_live":false}"""

    private fun store(name: String = "session") =
        SessionStore(PreferenceDataStoreFactory.create(scope = scope) { File(dir, "$name.preferences_pb") })

    private fun json(body: String, code: Int = 200) =
        MockResponse().setResponseCode(code).setHeader("Content-Type", "application/json").setBody(body)

    @Before
    fun setUp() {
        server = MockWebServer().apply { start() }
        dir = Files.createTempDirectory("reel-session").toFile()
    }

    @After
    fun tearDown() {
        server.shutdown()
        scope.cancel()
        dir.deleteRecursively()
    }

    @Test
    fun connectSignInUnauthorizedAndSignOut(): Unit = runBlocking {
        val st = store()
        val m = SessionManager(st, http, scope, deviceName = "Pixel", defaultServer = "https://preset.example")
        m.start()
        assertEquals(AuthState.NeedsServer("https://preset.example"), m.state.value)

        server.enqueue(json("""{"status":"ok"}"""))
        server.enqueue(json("""{"needs_setup":false}"""))
        val address = m.connect(server.url("/").toString())
        assertTrue(m.state.value is AuthState.NeedsSignIn)

        server.enqueue(json("""{"token":"tok","expires_at":1900000000,"user":$me}"""))
        m.signIn(" rafik ", "pw")
        assertEquals("rafik", (m.state.value as AuthState.SignedIn).user.username)
        val saved = st.current()
        assertEquals("tok", saved.token)
        assertEquals(address, saved.server)

        // A 401 on any signed-in call ends the session and lands on sign-in with a notice.
        server.enqueue(json("""{"error":"unauthorized"}""", 401))
        runCatching { m.api!!.me() }
        withTimeout(5_000) { while (m.state.value !is AuthState.NeedsSignIn) kotlinx.coroutines.delay(10) }
        assertEquals("Your session has ended. Please sign in again.", (m.state.value as AuthState.NeedsSignIn).notice)
        assertNull(st.current().token)
        assertEquals("the server is kept", address, st.current().server)
    }

    @Test
    fun loginSendsAStableDeviceId(): Unit = runBlocking {
        val st = store()
        val m = SessionManager(st, http, scope, deviceName = "Pixel")
        m.start()
        server.enqueue(json("""{"status":"ok"}"""))
        server.enqueue(json("""{"needs_setup":false}"""))
        m.connect(server.url("/").toString())
        repeat(2) {
            server.enqueue(json("""{"token":"tok$it","expires_at":1900000000,"user":$me}"""))
            m.signIn("rafik", "pw")
        }
        server.takeRequest(); server.takeRequest()
        val first = server.takeRequest().body.readUtf8()
        val second = server.takeRequest().body.readUtf8()
        val id = Regex(""""device_id":"([^"]+)"""").find(first)!!.groupValues[1]
        assertTrue(second.contains(id))
        assertEquals(id, st.deviceId())
        assertTrue(first.contains(""""device_name":"Pixel""""))
    }

    @Test
    fun aStoredSessionOpensSignedInAndSignOutRevokesIt(): Unit = runBlocking {
        val st = store()
        st.setServer(ServerAddress.parse(server.url("/").toString())!!)
        st.signedIn("tok", ReelJson.decodeFromString(Me.serializer(), me))
        val m = SessionManager(st, http, scope, deviceName = "Pixel")
        server.enqueue(json(me)) // background /me refresh
        m.start()
        assertTrue(m.state.value is AuthState.SignedIn)
        assertEquals("/api/v1/me", server.takeRequest().path)

        server.enqueue(MockResponse().setResponseCode(204))
        m.signOut()
        val logout = server.takeRequest()
        assertEquals("/api/v1/auth/logout", logout.path)
        assertEquals("Bearer tok", logout.getHeader("Authorization"))
        assertTrue(m.state.value is AuthState.NeedsSignIn)
        assertNull(st.session.first().token)
    }

    @Test
    fun connectRejectsServersThatArentReady(): Unit = runBlocking {
        val m = SessionManager(store(), http, scope, deviceName = "Pixel")
        m.start()
        server.enqueue(json("""{"status":"ok"}"""))
        server.enqueue(json("""{"needs_setup":true}"""))
        val e = runCatching { m.connect(server.url("/").toString()) }.exceptionOrNull()
        assertTrue(e!!.message!!.contains("isn't set up yet"))
        server.enqueue(MockResponse().setResponseCode(404))
        server.enqueue(MockResponse().setResponseCode(404))
        val e2 = runCatching { m.connect(server.url("/").toString()) }.exceptionOrNull()
        assertTrue(e2!!.message!!.contains("doesn't look like a Reel server"))
        assertTrue(m.state.value is AuthState.NeedsServer)
    }

    @Test
    fun changingServerDropsTheOldToken(): Unit = runBlocking {
        val st = store()
        st.setServer(ServerAddress.parse("http://a:8420")!!)
        st.signedIn("tok", ReelJson.decodeFromString(Me.serializer(), me))
        st.setServer(ServerAddress.parse("http://a:8420")!!)
        assertEquals("tok", st.current().token)
        st.setServer(ServerAddress.parse("http://b:8420")!!)
        assertNull(st.current().token)
    }
}

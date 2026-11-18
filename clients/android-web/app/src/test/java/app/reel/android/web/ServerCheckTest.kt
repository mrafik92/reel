package app.reel.android.web

import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ServerCheckTest {
    private val mock = MockWebServer().apply { start() }
    private val address get() = ServerAddress.parse("http://${mock.hostName}:${mock.port}")!!

    @After fun stop() = mock.shutdown()

    @Test fun reelHealthIsOk() {
        mock.enqueue(MockResponse().setBody("""{"status":"ok"}"""))
        assertEquals(CheckResult.Ok, ServerCheck().check(address))
        assertEquals("/api/v1/health", mock.takeRequest().path)
    }

    @Test fun anotherWebServerIsNotReel() {
        mock.enqueue(MockResponse().setBody("<html>router login</html>"))
        assertEquals(CheckResult.NotReel, ServerCheck().check(address))
        mock.enqueue(MockResponse().setResponseCode(404))
        assertEquals(CheckResult.NotReel, ServerCheck().check(address))
    }

    @Test fun closedPortIsUnreachable() {
        val a = address
        mock.shutdown()
        assertTrue(ServerCheck().check(a) is CheckResult.Unreachable)
    }
}

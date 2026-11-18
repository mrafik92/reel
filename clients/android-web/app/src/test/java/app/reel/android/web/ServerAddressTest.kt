package app.reel.android.web

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ServerAddressTest {
    private fun origin(input: String) = ServerAddress.parse(input)?.origin

    @Test fun lanAndTailscaleAddressesGetHttp() {
        assertEquals("http://192.168.1.20:8420", origin("192.168.1.20:8420"))
        assertEquals("http://100.101.102.103:8420", origin("100.101.102.103:8420"))
        assertEquals("http://nas:8420", origin("nas:8420"))
        assertEquals("http://reel.local", origin("reel.local"))
        assertEquals("http://reel.tail1234.ts.net:8420", origin("reel.tail1234.ts.net:8420"))
    }

    @Test fun dottedNamesGetHttps() {
        assertEquals("https://reel.example.com", origin("reel.example.com"))
        assertEquals("https://reel.tail1234.ts.net", origin("reel.tail1234.ts.net"))
    }

    @Test fun keepsOnlyTheOrigin() {
        assertEquals("https://reel.example.com", origin("https://reel.example.com/library/3?x=1"))
        assertEquals("http://nas:8420", origin(" http://nas:8420/ "))
        assertEquals("http://[fd7a:115c:a1e0::1]:8420", origin("[fd7a:115c:a1e0::1]:8420"))
    }

    @Test fun rejectsNonWebInput() {
        assertNull(origin(""))
        assertNull(origin("ftp://nas"))
        assertNull(origin("two words"))
        assertNull(origin("javascript:alert(1)"))
    }

    @Test fun homeAndHealthAreOnTheOrigin() {
        val s = ServerAddress.parse("nas:8420")!!
        assertEquals("http://nas:8420/", s.home)
        assertEquals("http://nas:8420/api/v1/health", s.health.toString())
    }
}

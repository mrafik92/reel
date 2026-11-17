package app.reel.android.data

import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ServerAddressTest {
    private fun origin(s: String) = ServerAddress.parse(s)?.origin

    @Test
    fun guessesTheScheme() {
        assertEquals("https://reel.example.com", origin("reel.example.com"))
        assertEquals("http://192.168.1.5:8420", origin("192.168.1.5:8420"))
        assertEquals("http://192.168.1.5", origin("192.168.1.5"))
        assertEquals("http://nas:8420", origin("nas:8420"))
        assertEquals("http://nas", origin("nas"))
        assertEquals("http://reel.local", origin("reel.local"))
        assertEquals("http://reel.example.com:8420", origin("reel.example.com:8420"))
        assertEquals("http://[fd7a::1]:8420", origin("[fd7a::1]:8420"))
    }

    @Test
    fun keepsOnlyTheOrigin() {
        assertEquals("https://reel.example.com", origin("  https://reel.example.com/library/3?x=1 "))
        assertEquals("https://reel.example.com", origin("https://reel.example.com:443/"))
        assertEquals("http://nas:8420", origin("HTTP://NAS:8420/api/v1"))
        assertEquals("https://a.b:8443", origin("https://a.b:8443"))
    }

    @Test
    fun rejectsNonsense() {
        assertNull(origin(""))
        assertNull(origin("   "))
        assertNull(origin("ftp://x.y"))
        assertNull(origin("two words"))
        assertNull(origin("http://"))
    }

    @Test
    fun apiPathsAndOwnership() {
        val s = ServerAddress.parse("http://nas:8420")!!
        assertEquals("http://nas:8420/api/v1/me", s.api("/me").toString())
        assertTrue(s.owns("http://nas:8420/api/v1/items/3".toHttpUrl()))
        assertFalse(s.owns("http://nas:8421/api/v1/me".toHttpUrl()))
        assertFalse(s.owns("https://nas:8420/api/v1/me".toHttpUrl()))
        assertFalse(s.owns("http://image.tmdb.org/t/p/w500/x.jpg".toHttpUrl()))
    }
}

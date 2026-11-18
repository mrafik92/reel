package app.reel.android.web

import app.reel.android.web.PageKeys.Unhandled
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class PageKeysTest {
    @Test fun downTimesNeverRepeat() {
        val keys = PageKeys()
        val a = keys.nextDownTime(100)
        val b = keys.nextDownTime(100)
        val c = keys.nextDownTime(99)
        assertNotEquals(a, b)
        assertEquals(b + 1, c)
        assertEquals(500, keys.nextDownTime(500))
    }

    @Test fun anIgnoredBackFallsBackOnceAndItsKeyUpIsSwallowed() {
        val keys = PageKeys()
        keys.sent(10, fallback = true)
        assertEquals(Unhandled.FALLBACK, keys.unhandled(10, isDown = true))
        assertEquals(Unhandled.SWALLOW, keys.unhandled(10, isDown = false))
        // Forgotten after its key-up.
        assertEquals(Unhandled.NOT_OURS, keys.unhandled(10, isDown = true))
    }

    @Test fun theFieldEscapeNeverFallsBack() {
        val keys = PageKeys()
        keys.sent(10, fallback = false)
        keys.sent(11, fallback = true)
        assertEquals(Unhandled.SWALLOW, keys.unhandled(10, isDown = true))
        assertEquals(Unhandled.SWALLOW, keys.unhandled(10, isDown = false))
        assertEquals(Unhandled.FALLBACK, keys.unhandled(11, isDown = true))
    }

    @Test fun aHandledBackOnlyReturnsItsKeyUp() {
        val keys = PageKeys()
        keys.sent(10, fallback = true)
        assertEquals(Unhandled.SWALLOW, keys.unhandled(10, isDown = false))
    }

    @Test fun otherKeysAreNotOurs() {
        val keys = PageKeys()
        keys.sent(10, fallback = true)
        assertEquals(Unhandled.NOT_OURS, keys.unhandled(42, isDown = true))
    }

    @Test fun lostKeyUpsDontPileUp() {
        val keys = PageKeys(capacity = 2)
        keys.sent(1, fallback = true)
        keys.sent(2, fallback = true)
        keys.sent(3, fallback = true)
        assertEquals(Unhandled.NOT_OURS, keys.unhandled(1, isDown = true))
        assertEquals(Unhandled.FALLBACK, keys.unhandled(3, isDown = true))
    }
}

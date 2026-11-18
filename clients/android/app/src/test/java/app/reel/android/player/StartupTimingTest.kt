package app.reel.android.player

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StartupTimingTest {
    @Test
    fun emitsOnceAndRejectsLateEvents() {
        var clock = 100L
        val lines = mutableListOf<String>()
        val timing = StartupTiming(7, clock, false, { clock }, lines::add)
        clock = 110
        timing.mark(StartupTiming.Stage.route)
        clock = 120
        timing.decided("direct_play")
        timing.finish(StartupTiming.Outcome.canceled)
        timing.mark(StartupTiming.Stage.frame)
        timing.finish(StartupTiming.Outcome.rendered)
        assertEquals(1, lines.size)
        assertTrue(lines.single().contains("outcome=canceled"))
        assertTrue(lines.single().contains("route_ms=10"))
        assertFalse(lines.single().contains("frame_ms"))
    }

    @Test
    fun unexpectedModeIsNeverLogged() {
        val lines = mutableListOf<String>()
        val timing = StartupTiming(1, 1, true, { 2 }, lines::add)
        timing.decided("https://example.invalid/private")
        timing.finish(StartupTiming.Outcome.failed)
        assertTrue(lines.single().contains("mode=unknown"))
        assertFalse(lines.single().contains("example"))
    }
}

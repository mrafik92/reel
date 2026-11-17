package app.reel.android.player

import android.view.KeyEvent
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.MediaFile
import app.reel.android.ui.shell.Routes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PlaybackHelpersTest {
    private fun episode(id: Long, no: Long, progress: Double? = null) =
        ItemDetail(id = id, kind = "episode", title = "E$no", episodeNo = no, seasonNo = 1, parentId = 9, progress = progress,
            files = listOf(MediaFile(id = id * 10, container = "mkv", durationS = 1000.0, size = 1)))

    private fun summary(id: Long, no: Long?, kind: String = "episode") = ItemSummary(id = id, kind = kind, title = "x", episodeNo = no)

    @Test
    fun resume() {
        assertEquals(250.0, resumePositionS(episode(1, 1, 0.25)), 1e-9)
        assertEquals(0.0, resumePositionS(episode(1, 1, 0.96)), 0.0)
        assertEquals(0.0, resumePositionS(episode(1, 1, null)), 0.0)
    }

    @Test
    fun nextEpisodeIsTheLowestHigherNumber() {
        val sibs = listOf(summary(1, 1), summary(4, 4), summary(3, 3), summary(2, 2), summary(5, null))
        assertEquals(3L, nextEpisode(episode(2, 2), sibs)?.id)
        assertNull(nextEpisode(episode(4, 4), sibs))
        assertNull(nextEpisode(episode(2, 2).copy(kind = "movie"), sibs))
    }

    @Test
    fun keys() {
        assertEquals(KeyAction.Seek(-10, focusSeekBar = true), PlayerKeys.map(KeyEvent.KEYCODE_DPAD_LEFT, controlsVisible = false))
        assertEquals(KeyAction.Pass, PlayerKeys.map(KeyEvent.KEYCODE_DPAD_LEFT, controlsVisible = true))
        assertEquals(KeyAction.Wake, PlayerKeys.map(KeyEvent.KEYCODE_DPAD_CENTER, controlsVisible = false))
        assertEquals(KeyAction.Seek(30), PlayerKeys.map(KeyEvent.KEYCODE_MEDIA_FAST_FORWARD, controlsVisible = true))
        assertEquals(KeyAction.Seek(-10), PlayerKeys.map(KeyEvent.KEYCODE_MEDIA_REWIND, controlsVisible = false))
        assertEquals(KeyAction.TogglePlay, PlayerKeys.map(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, controlsVisible = true))
        assertEquals(KeyAction.Next, PlayerKeys.map(KeyEvent.KEYCODE_MEDIA_NEXT, controlsVisible = false))
        assertEquals(KeyAction.Pass, PlayerKeys.map(KeyEvent.KEYCODE_BACK, controlsVisible = false))
    }

    @Test
    fun labels() {
        assertEquals("English", subtitleLabel("eng", null, false))
        assertEquals("English · SDH (forced)", subtitleLabel("eng", "SDH", true))
        assertEquals("Unknown", subtitleLabel("und", null, false))
        assertEquals("1:01:01", formatTime(3_661_000))
        assertEquals("0:09", formatTime(9_400))
        assertEquals("S1 E2", episodeCode(1, 2))
        assertEquals(true, Routes.isFullScreen(Routes.PLAYER))
        assertEquals(false, Routes.isFullScreen(Routes.ITEM))
    }
}

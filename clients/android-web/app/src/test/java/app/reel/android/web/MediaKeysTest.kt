package app.reel.android.web

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class MediaKeysTest {
    @Test fun theHeadsetButtonPlaysAndPauses() {
        assertEquals(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, MediaKeys.forPage(KeyEvent.KEYCODE_HEADSETHOOK))
    }

    @Test fun keysWebViewAlreadyMapsPassThrough() {
        listOf(
            KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE,
            KeyEvent.KEYCODE_MEDIA_PLAY,
            KeyEvent.KEYCODE_MEDIA_PAUSE,
            KeyEvent.KEYCODE_MEDIA_FAST_FORWARD,
            KeyEvent.KEYCODE_MEDIA_REWIND,
            KeyEvent.KEYCODE_ESCAPE,
            KeyEvent.KEYCODE_BACK,
            KeyEvent.KEYCODE_DPAD_CENTER,
        ).forEach { assertNull("key $it", MediaKeys.forPage(it)) }
    }
}

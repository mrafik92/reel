package app.reel.android.web

import android.content.pm.ActivityInfo
import org.junit.Assert.assertEquals
import org.junit.Test

class FullScreenTest {
    @Test fun phonesTurnToLandscape() {
        assertEquals(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE, FullScreen.orientationFor(tv = false, current = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED))
        assertEquals(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE, FullScreen.orientationFor(tv = false, current = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT))
    }

    @Test fun tvsKeepTheirOrientation() {
        assertEquals(ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED, FullScreen.orientationFor(tv = true, current = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED))
    }
}

package app.reel.android

import android.content.res.Configuration
import org.junit.Assert.assertEquals
import org.junit.Test

class DeviceKindTest {
    @Test
    fun televisionModeIsTv() {
        val uiMode = Configuration.UI_MODE_TYPE_TELEVISION or Configuration.UI_MODE_NIGHT_YES
        assertEquals(DeviceKind.Tv, DeviceKind.fromUiMode(uiMode))
    }

    @Test
    fun everythingElseIsPhone() {
        for (type in listOf(
            Configuration.UI_MODE_TYPE_NORMAL,
            Configuration.UI_MODE_TYPE_DESK,
            Configuration.UI_MODE_TYPE_CAR,
            Configuration.UI_MODE_TYPE_UNDEFINED,
        )) {
            assertEquals(DeviceKind.Phone, DeviceKind.fromUiMode(type))
        }
    }
}

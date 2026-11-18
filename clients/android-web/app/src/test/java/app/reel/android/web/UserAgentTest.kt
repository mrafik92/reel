package app.reel.android.web

import org.junit.Assert.assertEquals
import org.junit.Test

class UserAgentTest {
    private val base = "Mozilla/5.0 (Linux; Android 14; Pixel 7; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/130.0.0.0 Mobile Safari/537.36"

    @Test fun appendsTheToken() {
        assertEquals("$base ReelAndroidWeb/0.1.0", UserAgent.extend(base, "0.1.0", tv = false))
        assertEquals("$base ReelAndroidWeb/0.1.0 (TV)", UserAgent.extend(base, "0.1.0", tv = true))
    }

    @Test fun replacesAnEarlierToken() {
        val once = UserAgent.extend(base, "0.1.0", tv = true)
        assertEquals("$base ReelAndroidWeb/0.2.0", UserAgent.extend(once, "0.2.0", tv = false))
    }
}

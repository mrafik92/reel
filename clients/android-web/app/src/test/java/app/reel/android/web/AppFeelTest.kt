package app.reel.android.web

import android.webkit.WebView.HitTestResult
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AppFeelTest {
    @Test fun longPressOnlyInTextFields() {
        assertTrue(AppFeel.allowsLongPress(HitTestResult.EDIT_TEXT_TYPE))
        assertFalse(AppFeel.allowsLongPress(HitTestResult.UNKNOWN_TYPE)) // plain text
        assertFalse(AppFeel.allowsLongPress(HitTestResult.SRC_ANCHOR_TYPE)) // a link
        assertFalse(AppFeel.allowsLongPress(HitTestResult.IMAGE_TYPE))
        assertFalse(AppFeel.allowsLongPress(HitTestResult.SRC_IMAGE_ANCHOR_TYPE))
    }
}

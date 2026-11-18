package app.reel.android.web

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BackPolicyTest {
    private val server = ServerAddress.parse("https://reel.example.com")!!

    @Test fun fullScreenExitsFirst() {
        assertEquals(BackAction.EXIT_FULL_SCREEN, BackPolicy.onBack(fullScreen = true, errorShown = false, hasPage = true, canGoBack = true))
        assertEquals(BackAction.EXIT_FULL_SCREEN, BackPolicy.onBack(fullScreen = true, errorShown = true, hasPage = true, canGoBack = false))
    }

    @Test fun thePageDecidesNext() {
        // Even when WebView reports no history (entries made without a gesture are skipped).
        assertEquals(BackAction.ASK_PAGE, BackPolicy.onBack(fullScreen = false, errorShown = false, hasPage = true, canGoBack = false))
        assertEquals(BackAction.ASK_PAGE, BackPolicy.onBack(fullScreen = false, errorShown = false, hasPage = true, canGoBack = true))
    }

    @Test fun theErrorPanelUsesNativeHistory() {
        assertEquals(BackAction.GO_BACK, BackPolicy.onBack(fullScreen = false, errorShown = true, hasPage = true, canGoBack = true))
        assertEquals(BackAction.LEAVE, BackPolicy.onBack(fullScreen = false, errorShown = true, hasPage = true, canGoBack = false))
    }

    @Test fun noPageLeaves() {
        assertEquals(BackAction.LEAVE, BackPolicy.onBack(fullScreen = false, errorShown = false, hasPage = false, canGoBack = false))
    }

    @Test fun anIgnoredBackLeavesAtTheRoot() {
        assertEquals(BackAction.LEAVE, BackPolicy.whenPageIgnored(canGoBack = true, atRoot = true))
        assertEquals(BackAction.LEAVE, BackPolicy.whenPageIgnored(canGoBack = false, atRoot = false))
        assertEquals(BackAction.GO_BACK, BackPolicy.whenPageIgnored(canGoBack = true, atRoot = false))
    }

    @Test fun rootIsSlashOnTheServer() {
        assertTrue(BackPolicy.isRoot(server, "https://reel.example.com/"))
        assertTrue(BackPolicy.isRoot(server, "https://reel.example.com"))
        assertTrue(BackPolicy.isRoot(server, "https://reel.example.com/?tv=1#x"))
        assertFalse(BackPolicy.isRoot(server, "https://reel.example.com/search?q=alien"))
        assertFalse(BackPolicy.isRoot(server, "https://reel.example.com/login"))
        assertFalse(BackPolicy.isRoot(server, "https://other.example.com/"))
        assertFalse(BackPolicy.isRoot(server, "about:blank"))
        assertFalse(BackPolicy.isRoot(server, null))
    }
}

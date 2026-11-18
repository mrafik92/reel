package app.reel.android.web

import android.webkit.WebViewClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RecoveryTest {
    private fun classify(code: Int, description: String?, online: Boolean = true) =
        LoadErrors.classify(code, description, online)

    @Test fun theCopiedCodesMatchWebView() {
        assertEquals(WebViewClient.ERROR_HOST_LOOKUP, LoadErrors.ERROR_HOST_LOOKUP)
        assertEquals(WebViewClient.ERROR_CONNECT, LoadErrors.ERROR_CONNECT)
        assertEquals(WebViewClient.ERROR_TIMEOUT, LoadErrors.ERROR_TIMEOUT)
        assertEquals(WebViewClient.ERROR_FAILED_SSL_HANDSHAKE, LoadErrors.ERROR_FAILED_SSL_HANDSHAKE)
    }

    @Test fun withoutANetworkEverythingIsOffline() {
        assertEquals(LoadProblem.OFFLINE, classify(LoadErrors.ERROR_HOST_LOOKUP, "net::ERR_NAME_NOT_RESOLVED", online = false))
        assertEquals(LoadProblem.OFFLINE, classify(LoadErrors.ERROR_CONNECT, "net::ERR_CONNECTION_REFUSED", online = false))
        assertEquals(LoadProblem.OFFLINE, classify(-1, null, online = false))
    }

    @Test fun internetDisconnectedIsOfflineEvenIfAnInterfaceIsUp() {
        assertEquals(LoadProblem.OFFLINE, classify(LoadErrors.ERROR_HOST_LOOKUP, "net::ERR_INTERNET_DISCONNECTED"))
    }

    @Test fun nameErrors() {
        assertEquals(LoadProblem.UNKNOWN_HOST, classify(LoadErrors.ERROR_HOST_LOOKUP, "net::ERR_NAME_NOT_RESOLVED"))
        assertEquals(LoadProblem.UNKNOWN_HOST, classify(-1, "net::ERR_NAME_RESOLUTION_FAILED"))
        // Only the code, no description.
        assertEquals(LoadProblem.UNKNOWN_HOST, classify(LoadErrors.ERROR_HOST_LOOKUP, null))
    }

    @Test fun refused() {
        assertEquals(LoadProblem.REFUSED, classify(LoadErrors.ERROR_CONNECT, "net::ERR_CONNECTION_REFUSED"))
    }

    @Test fun aPortChromiumRefusesToUse() {
        assertEquals(LoadProblem.BLOCKED_PORT, classify(-1, "net::ERR_UNSAFE_PORT"))
        assertFalse(LoadProblem.BLOCKED_PORT.autoRetry)
    }

    @Test fun timeouts() {
        assertEquals(LoadProblem.TIMED_OUT, classify(LoadErrors.ERROR_TIMEOUT, "net::ERR_CONNECTION_TIMED_OUT"))
        assertEquals(LoadProblem.TIMED_OUT, classify(LoadErrors.ERROR_TIMEOUT, "net::ERR_TIMED_OUT"))
        assertEquals(LoadProblem.TIMED_OUT, classify(LoadErrors.ERROR_CONNECT, "net::ERR_ADDRESS_UNREACHABLE"))
        assertEquals(LoadProblem.TIMED_OUT, classify(LoadErrors.ERROR_TIMEOUT, null))
    }

    @Test fun tls() {
        assertEquals(LoadProblem.TLS, classify(LoadErrors.ERROR_FAILED_SSL_HANDSHAKE, "net::ERR_SSL_PROTOCOL_ERROR"))
        assertEquals(LoadProblem.TLS, classify(-1, "net::ERR_CERT_AUTHORITY_INVALID"))
        assertEquals(LoadProblem.TLS, classify(LoadErrors.ERROR_FAILED_SSL_HANDSHAKE, null))
    }

    @Test fun everythingElseIsAGenericNetworkFailure() {
        assertEquals(LoadProblem.NETWORK, classify(LoadErrors.ERROR_CONNECT, "net::ERR_CONNECTION_RESET"))
        assertEquals(LoadProblem.NETWORK, classify(-1, "net::ERR_EMPTY_RESPONSE"))
        assertEquals(LoadProblem.NETWORK, classify(-1, "something WebView made up"))
        assertEquals(LoadProblem.NETWORK, classify(-1, null))
    }

    @Test fun theNetErrorNameIsTheDetailLine() {
        assertEquals("net::ERR_NAME_NOT_RESOLVED", LoadErrors.netError("net::ERR_NAME_NOT_RESOLVED"))
        assertEquals("net::ERR_CONNECTION_REFUSED", LoadErrors.netError("Failed: net::ERR_CONNECTION_REFUSED."))
        assertNull(LoadErrors.netError("no code here"))
        assertNull(LoadErrors.netError(null))
    }

    @Test fun onlyServerErrorsTakeOverTheScreen() {
        assertEquals(LoadProblem.SERVER, LoadErrors.fromHttpStatus(500))
        assertEquals(LoadProblem.SERVER, LoadErrors.fromHttpStatus(502))
        assertEquals(LoadProblem.SERVER, LoadErrors.fromHttpStatus(503))
        // A 404 or 401 page is the web app's (its router or sign-in handle it).
        assertNull(LoadErrors.fromHttpStatus(404))
        assertNull(LoadErrors.fromHttpStatus(401))
        assertNull(LoadErrors.fromHttpStatus(200))
    }

    @Test fun theTimeoutIsWellBelowChromiumsAndAboveASlowAnswer() {
        assertTrue(LoadErrors.LOAD_TIMEOUT_MS in 10_000L..30_000L)
    }

    @Test fun onlyNetworkAndServerProblemsAreRetriedAutomatically() {
        val manual = setOf(LoadProblem.TLS, LoadProblem.CRASH, LoadProblem.BLOCKED_PORT)
        for (p in LoadProblem.entries) assertEquals(p.name, p !in manual, p.autoRetry)
    }

    @Test fun aWorkingPageIsNeverReloaded() {
        assertEquals(RetryStep.Idle, RetryPolicy.next(problem = null, online = true, attempt = 0))
        assertEquals(RetryStep.Idle, RetryPolicy.next(problem = null, online = false, attempt = 0))
    }

    @Test fun offlineWaitsForTheNetwork() {
        assertEquals(RetryStep.WaitForNetwork, RetryPolicy.next(LoadProblem.OFFLINE, online = false, attempt = 0))
        assertEquals(RetryStep.WaitForNetwork, RetryPolicy.next(LoadProblem.TIMED_OUT, online = false, attempt = 3))
    }

    @Test fun retriesBackOffThenStop() {
        val delays = generateSequence(0) { it + 1 }
            .map { RetryPolicy.next(LoadProblem.TIMED_OUT, online = true, attempt = it) }
            .takeWhile { it is RetryStep.RetryIn }
            .map { (it as RetryStep.RetryIn).delayMs }
            .toList()
        assertEquals(listOf(1_000L, 2_000L, 4_000L, 8_000L, 16_000L, 32_000L), delays)
        assertEquals(RetryStep.Idle, RetryPolicy.next(LoadProblem.TIMED_OUT, online = true, attempt = delays.size))
    }

    @Test fun tlsWaitsForTheUser() {
        assertEquals(RetryStep.Idle, RetryPolicy.next(LoadProblem.TLS, online = true, attempt = 0))
        assertEquals(RetryStep.Idle, RetryPolicy.next(LoadProblem.CRASH, online = true, attempt = 0))
    }
}

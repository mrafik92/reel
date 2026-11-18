package app.reel.android.web

import org.junit.Assert.assertEquals
import org.junit.Test

class LinkPolicyTest {
    private val server = ServerAddress.parse("http://192.168.1.20:8420")!!
    private fun target(url: String) = LinkPolicy.classify(server, url)

    @Test fun sameOriginStaysInApp() {
        assertEquals(LinkTarget.IN_APP, target("http://192.168.1.20:8420/"))
        assertEquals(LinkTarget.IN_APP, target("http://192.168.1.20:8420/search?q=alien"))
        assertEquals(LinkTarget.IN_APP, target("HTTP://192.168.1.20:8420/item/3"))
    }

    @Test fun otherOriginsOpenOutside() {
        assertEquals(LinkTarget.EXTERNAL, target("https://www.themoviedb.org/movie/1"))
        assertEquals(LinkTarget.EXTERNAL, target("http://192.168.1.20:8421/")) // other port
        assertEquals(LinkTarget.EXTERNAL, target("https://192.168.1.20:8420/")) // other scheme
        assertEquals(LinkTarget.EXTERNAL, target("http://192.168.1.21:8420/")) // other host
        assertEquals(LinkTarget.EXTERNAL, target("http://192.168.1.20.evil.example:8420/"))
        assertEquals(LinkTarget.EXTERNAL, target("mailto:someone@example.com"))
        assertEquals(LinkTarget.EXTERNAL, target("tel:+15551234"))
    }

    @Test fun everythingElseIsBlocked() {
        assertEquals(LinkTarget.BLOCKED, target("intent://scan/#Intent;scheme=zxing;package=com.example;end"))
        assertEquals(LinkTarget.BLOCKED, target("javascript:alert(1)"))
        assertEquals(LinkTarget.BLOCKED, target("file:///sdcard/secret.txt"))
        assertEquals(LinkTarget.BLOCKED, target("content://com.example.provider/x"))
        assertEquals(LinkTarget.BLOCKED, target("data:text/html,<b>hi</b>"))
        assertEquals(LinkTarget.BLOCKED, target("market://details?id=x"))
        assertEquals(LinkTarget.BLOCKED, target("no scheme at all"))
    }

    @Test fun userInfoTricksDontPassAsTheServer() {
        assertEquals(LinkTarget.EXTERNAL, target("http://192.168.1.20:8420@evil.example/"))
    }
}

package app.reel.android.ui

import app.reel.android.data.Me
import app.reel.android.data.Permissions
import app.reel.android.data.ServerAddress
import app.reel.android.ui.common.resolveImageUrl
import app.reel.android.ui.shell.TopLevel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ShellTest {
    @Test
    fun liveTvOnlyForUsersWithAccess() {
        val user = Me(1, "u", "user")
        assertEquals(listOf("home", "search", "library", "settings"), TopLevel.visibleTo(user).map { it.route })
        val live = user.copy(permissions = Permissions(iptvAccess = true))
        assertEquals(5, TopLevel.visibleTo(live).size)
        assertEquals(5, TopLevel.visibleTo(Me(2, "a", "admin")).size)
    }

    @Test
    fun imageUrlsResolveAgainstTheServer() {
        val s = ServerAddress.parse("http://nas:8420")
        assertEquals("http://nas:8420/api/v1/images/3?sig=x", resolveImageUrl(s, "/api/v1/images/3?sig=x"))
        assertEquals("https://image.tmdb.org/t/p/w500/a.jpg", resolveImageUrl(s, "https://image.tmdb.org/t/p/w500/a.jpg"))
        assertNull(resolveImageUrl(s, null))
        assertNull(resolveImageUrl(s, ""))
        assertNull(resolveImageUrl(null, "/x"))
    }
}

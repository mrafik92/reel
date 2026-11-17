package app.reel.android.ui

import app.reel.android.data.Home
import app.reel.android.data.HomeRow
import app.reel.android.data.ItemSummary
import app.reel.android.ui.home.cardMeta
import app.reel.android.ui.home.featured
import app.reel.android.ui.home.lanes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HomeLanesTest {
    private fun item(id: Long) = ItemSummary(id, "movie", "M$id")

    @Test
    fun emptyRowsAreLeftOutAndLibraryRowsLinkToTheirLibrary() {
        val h = Home(
            continueWatching = listOf(item(1)),
            recentlyAdded = emptyList(),
            rows = listOf(HomeRow(5, "Movies", listOf(item(2))), HomeRow(6, "Empty", emptyList())),
        )
        val l = lanes(h)
        assertEquals(listOf("continue", "lib-5"), l.map { it.key })
        assertNull(l[0].libraryId)
        assertEquals(true, l[0].landscape)
        assertEquals(5L, l[1].libraryId)
        assertEquals(1L, featured(h)?.id)
        assertEquals(2L, featured(h.copy(continueWatching = emptyList(), recentlyAdded = listOf(item(2))))?.id)
    }

    @Test
    fun cardMetaNamesTheEpisodeAndTimeLeft() {
        val e = ItemSummary(1, "episode", "Low Water", seasonNo = 2, episodeNo = 5, progress = 0.52, runtimeS = 3000, year = 2024)
        assertEquals("S2 · E5 · 24 min left", cardMeta(e))
        val m = ItemSummary(2, "movie", "M", year = 2019, genres = listOf("Drama", "Mystery", "War"))
        assertEquals("Drama · Mystery · 2019", cardMeta(m, withGenres = true))
    }
}

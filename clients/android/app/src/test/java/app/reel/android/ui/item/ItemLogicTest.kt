package app.reel.android.ui.item

import app.reel.android.data.AudioStream
import app.reel.android.data.ItemDetail
import app.reel.android.data.ItemSummary
import app.reel.android.data.MediaFile
import app.reel.android.data.VideoStream
import app.reel.android.ui.shell.PlayerArgs
import app.reel.android.ui.shell.Routes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ItemLogicTest {
    @Test
    fun resumePoint() {
        assertEquals(600.0, ItemLogic.resumeAt(0.25, 2400.0, played = false), 0.001)
        assertEquals(0.0, ItemLogic.resumeAt(0.25, 2400.0, played = true), 0.0)
        assertEquals(0.0, ItemLogic.resumeAt(0.001, 2400.0, played = false), 0.0) // barely started
        assertEquals(0.0, ItemLogic.resumeAt(0.97, 2400.0, played = false), 0.0) // as good as finished
        assertEquals(0.0, ItemLogic.resumeAt(null, 2400.0, played = false), 0.0)
        assertEquals(0.0, ItemLogic.resumeAt(0.5, 0.0, played = false), 0.0)
    }

    @Test
    fun clockAndRuntime() {
        assertEquals("0:00", ItemLogic.clock(0.0))
        assertEquals("12:05", ItemLogic.clock(725.4))
        assertEquals("1:02:03", ItemLogic.clock(3723.0))
        assertEquals("45 min", ItemLogic.runtime(2700))
        assertEquals("2 h", ItemLogic.runtime(7200))
        assertEquals("1 h 52 min", ItemLogic.runtime(6720))
    }

    private fun ep(id: Long, played: Boolean = false, progress: Double? = null) =
        ItemSummary(id, "episode", "E$id", played = played, progress = progress, seasonNo = 1, episodeNo = id)

    @Test
    fun nextUpPrefersInProgressThenUnwatched() {
        assertEquals(3L, ItemLogic.nextUp(listOf(ep(1, played = true), ep(2), ep(3, progress = 0.4)))?.id)
        assertEquals(2L, ItemLogic.nextUp(listOf(ep(1, played = true), ep(2), ep(3)))?.id)
        assertEquals(1L, ItemLogic.nextUp(listOf(ep(1, played = true), ep(2, played = true)))?.id)
        assertNull(ItemLogic.nextUp(emptyList()))
        assertEquals(2L, ItemLogic.defaultSeason(listOf(ep(1, played = true), ep(2)))?.id)
    }

    @Test
    fun metaLine() {
        val movie = ItemDetail(
            1, "movie", "M", year = 2010, contentRating = "PG-13", rating = 7.84, genres = listOf("Drama", "Sci-Fi", "War", "x"),
            files = listOf(MediaFile(9, "mkv", 6720.0, 1)),
        )
        assertEquals(listOf("2010", "PG-13", "1 h 52 min", "Drama / Sci-Fi / War"), ItemLogic.meta(movie))
        assertEquals("★ 7.8", ItemLogic.stars(movie.rating))
        assertNull(ItemLogic.stars(0.0))
        val show = ItemDetail(2, "show", "S", childrenCount = 3)
        assertEquals(listOf("3 seasons"), ItemLogic.meta(show))
        assertEquals("S2 · E5", ItemLogic.kindLabel(ItemDetail(3, "episode", "E", seasonNo = 2, episodeNo = 5)))
        assertEquals("E4 · 45 min", ItemLogic.episodeCaption(ep(4).copy(runtimeS = 2700)))
    }

    @Test
    fun timeLeftAndEpisodeMeta() {
        assertEquals("18 min left", ItemLogic.timeLeft(0.64, 3000.0))
        assertEquals("1 h 12 min left", ItemLogic.timeLeft(0.2, 5400.0))
        assertNull(ItemLogic.timeLeft(null, 3000.0))
        assertNull(ItemLogic.timeLeft(0.98, 3000.0))
        assertEquals("50 min · 18 min left", ItemLogic.episodeMeta(ep(3, progress = 0.64).copy(runtimeS = 3000)))
        assertEquals("52 min · Watched", ItemLogic.episodeMeta(ep(1, played = true).copy(runtimeS = 3120)))
    }

    @Test
    fun fileBadges() {
        val uhd = MediaFile(
            1, "mkv", 1.0, 1,
            video = VideoStream("hevc", 3840, 2160, hdr = "hdr10", bitDepth = 10),
            audio = listOf(AudioStream(1, "eac3", channels = 6), AudioStream(2, "aac", channels = 2)),
        )
        assertEquals(listOf("4K", "HDR10", "5.1"), ItemLogic.badges(uhd))
        val hd = MediaFile(2, "mp4", 1.0, 1, video = VideoStream("h264", 1920, 1080), audio = listOf(AudioStream(1, "truehd", channels = 8)))
        assertEquals(listOf("7.1"), ItemLogic.badges(hd))
        assertEquals(emptyList<String>(), ItemLogic.badges(null))
    }

    @Test
    fun playerRouteCarriesItsArguments() {
        // The contract with the player (#190): Player(fileId: Long, itemId: Long, startS: Double).
        assertEquals("player/{fileId}/{itemId}/{startS}?tap={tap}", Routes.PLAYER)
        assertEquals("player/9/42/600.5", Routes.player(9, 42, 600.5, 0))
    }

    @Test
    fun playerArgsDefaultsWithoutABundle() {
        assertEquals(PlayerArgs(0, 0, 0.0), PlayerArgs.from(null))
    }
}

package app.reel.android.ui.vod

import app.reel.android.data.VodEpisode
import org.junit.Assert.assertEquals
import org.junit.Test

class VodDetailTest {
    @Test
    fun primaryActionResumesAnInProgressEpisodeBeforeAnUnstartedOne() {
        val unstarted = VodEpisode(1, 1, "First", runtimeS = 1800)
        val watched = VodEpisode(2, 2, "Second", runtimeS = 1800, progress = 0.4, played = true)
        val partial = VodEpisode(3, 3, "Third", runtimeS = 1800, progress = 0.25)
        assertEquals(partial, vodNextEpisode(listOf(unstarted, watched, partial)))
        assertEquals(450.0, vodResumeS(partial.progress, partial.runtimeS, partial.played), 0.0)
        assertEquals(0.0, vodResumeS(watched.progress, watched.runtimeS, watched.played), 0.0)
    }
}

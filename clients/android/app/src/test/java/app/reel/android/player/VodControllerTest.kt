package app.reel.android.player

import app.reel.android.data.AudioStream
import app.reel.android.data.DecideResponse
import app.reel.android.data.PlaybackDecision
import app.reel.android.data.ProgressRequest
import app.reel.android.data.ReelError
import app.reel.android.data.Selected
import app.reel.android.data.StreamAction
import app.reel.android.data.SubtitleOption
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The VOD session state machine on virtual time, with a fake player and a fake server. */
@OptIn(ExperimentalCoroutinesApi::class)
class VodControllerTest {
    class FakePlayer : PlayerPort {
        override var listener: PlayerListener? = null
        val loads = mutableListOf<PlayItem>()
        var text: TextChoice? = null
        var audio: Int? = null
        var audioTracks = 0
        var embedded = 0
        var stops = 0
        override var positionMs = 0L
        override var durationMs: Long? = null
        override var playWhenReady = false

        override fun load(item: PlayItem) {
            loads += item
            positionMs = item.startMs
            playWhenReady = item.autoplay
        }

        override fun play() {
            playWhenReady = true
            listener?.onIsPlaying(true)
        }

        override fun pause() {
            playWhenReady = false
            listener?.onIsPlaying(false)
        }

        override fun seekTo(positionMs: Long) {
            this.positionMs = positionMs
        }

        override fun stop() {
            stops++
        }

        override fun audioTrackCount() = audioTracks

        override fun selectAudio(position: Int) {
            audio = position
        }

        override fun selectText(choice: TextChoice) {
            text = choice
        }

        override fun hasEmbeddedText(position: Int) = position < embedded

        /** The media started playing. */
        fun started() {
            listener?.onState(PlayerState.Ready)
            listener?.onIsPlaying(true)
        }
    }

    class FakeSource(var answer: () -> DecideResponse) : PlaySource {
        val opens = mutableListOf<OpenRequest>()
        val progress = mutableListOf<Pair<String, ProgressRequest>>()
        val stops = mutableListOf<String>()
        var progressError: Exception? = null
        var openError: Exception? = null
        var gate: CompletableDeferred<Unit>? = null

        override suspend fun open(o: OpenRequest): DecideResponse {
            opens += o
            gate?.await()
            openError?.let { throw it }
            return answer()
        }

        override suspend fun progress(sessionId: String, body: ProgressRequest) {
            progress += sessionId to body
            progressError?.let { throw it }
        }

        override suspend fun stop(sessionId: String) {
            stops += sessionId
        }
    }

    private var n = 0

    private fun session(
        mode: String = "direct_play",
        url: String? = null,
        audio: List<AudioStream> = listOf(AudioStream(1, "aac", default = true), AudioStream(2, "ac3", 6)),
        subs: List<SubtitleOption> = listOf(
            SubtitleOption(3, 30, language = "eng", url = "/api/v1/subs/3.vtt?sig=a"),
            SubtitleOption(4, 40, language = "fre"), // picture subtitle (no VTT)
        ),
        audioAction: String = "copy",
        subAction: String = "none",
        selectedSub: Long? = null,
    ): DecideResponse {
        n++
        return DecideResponse(
            sessionId = "s$n",
            mode = mode,
            url = url ?: if (mode == "direct_play") "/api/v1/direct/7?sig=x" else "/api/v1/stream/s$n/master.m3u8?sig=x",
            decision = PlaybackDecision(mode = mode, audio = StreamAction(audioAction), subtitle = StreamAction(subAction), video = StreamAction("copy")),
            audio = audio,
            subtitles = subs,
            selected = Selected("prefs", audioIndex = 1, subtitleIndex = selectedSub),
        )
    }

    private fun TestScope.controller(
        source: PlaySource,
        player: FakePlayer,
        durationS: Double? = 3600.0,
    ) = VodController(
        source = source,
        player = player,
        scope = backgroundScope,
        resolve = { "http://reel.test$it" },
        now = { testScheduler.currentTime },
        knownDurationS = durationS,
    )

    @Test
    fun opensAtTheResumePointAndLoadsTheAnswer() = runTest {
        val src = FakeSource { session(mode = "transcode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(120.0)
        runCurrent()
        assertEquals(OpenRequest(startS = 120.0, audioIndex = null, subtitleIndex = null, quality = "auto"), src.opens.single())
        val item = p.loads.single()
        assertEquals("http://reel.test/api/v1/stream/s1/master.m3u8?sig=x", item.url)
        assertEquals(PlayItem.MIME_HLS, item.mimeType)
        assertEquals(120_000L, item.startMs)
        assertEquals(listOf("reel-sub-3"), item.subtitles.map { it.id })
        assertEquals("http://reel.test/api/v1/subs/3.vtt?sig=a", item.subtitles[0].url)
        assertEquals(1L, c.state.value.audioIndex)
        assertEquals(-1L, c.state.value.subtitleIndex)
        assertEquals(TextChoice.Off, p.text)
    }

    @Test
    fun directPlayIsProgressiveAndPicksTheServersSubtitle() = runTest {
        val src = FakeSource { session(selectedSub = 3) }
        val p = FakePlayer()
        controller(src, p).start(0.0)
        runCurrent()
        assertNull(p.loads.single().mimeType)
        assertEquals(TextChoice.Sidecar("reel-sub-3"), p.text)
    }

    @Test
    fun aResumePointWithinFiveSecondsOfTheEndStartsOver() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        controller(src, p, durationS = 1000.0).start(997.0)
        runCurrent()
        assertEquals(0.0, src.opens.single().startS, 0.0)

        // Duration unknown up front: the player's duration decides.
        val src2 = FakeSource { session() }
        val p2 = FakePlayer()
        controller(src2, p2, durationS = null).start(997.0)
        runCurrent()
        assertEquals(997_000L, p2.loads.single().startMs)
        p2.listener!!.onDuration(1_000_000)
        assertEquals(0L, p2.positionMs)
        assertTrue(p2.playWhenReady)
    }

    @Test
    fun reportsProgressEveryTenSecondsAndOnPause() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(60.0)
        runCurrent()
        p.started()
        p.positionMs = 65_000
        advanceTimeBy(10_001)
        assertEquals(listOf("s1" to ProgressRequest(65.0, false)), src.progress)
        p.positionMs = 70_000
        advanceTimeBy(10_000)
        assertEquals(2, src.progress.size)

        c.pause()
        runCurrent()
        assertEquals(ProgressRequest(70.0, true), src.progress.last().second)
        // Paused: keep-alive pings carry paused = true.
        advanceTimeBy(10_000)
        assertEquals(ProgressRequest(70.0, true), src.progress.last().second)
        assertEquals(4, src.progress.size)
    }

    @Test
    fun noProgressInTheFirstSecond() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        controller(src, p).start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 500
        advanceTimeBy(10_001)
        assertTrue(src.progress.isEmpty())
    }

    @Test
    fun closeReportsAndEndsTheSession() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(30.0)
        runCurrent()
        p.started()
        p.positionMs = 42_000
        c.close()
        runCurrent()
        assertEquals("s1" to ProgressRequest(42.0, true), src.progress.last())
        assertEquals(listOf("s1"), src.stops)
        // No more ticks after close.
        advanceTimeBy(60_000)
        assertEquals(1, src.progress.size)
        c.close()
        runCurrent()
        assertEquals(1, src.stops.size)
    }

    @Test
    fun aSessionAnsweredAfterCloseIsStopped() = runTest {
        val src = FakeSource { session() }
        src.gate = CompletableDeferred()
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        c.close()
        src.gate!!.complete(Unit)
        runCurrent()
        assertTrue(p.loads.isEmpty())
    }

    @Test
    fun a404OnProgressDecidesAgainAtThePlayhead() = runTest {
        val src = FakeSource { session(mode = "transcode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 300_000
        src.progressError = ReelError.Http(404, "Not found")
        advanceTimeBy(10_001)
        assertEquals(2, src.opens.size)
        assertEquals(300.0, src.opens[1].startS, 0.0)
        assertEquals(listOf("s1"), src.stops)
        assertEquals("s2", c.state.value.session?.sessionId)
        assertEquals(300_000L, p.loads[1].startMs)
        assertNotNull(c.state.value.recovering)
        p.started()
        assertNull(c.state.value.recovering)
    }

    @Test
    fun a404WhilePausedWaitsForPlay() = runTest {
        val src = FakeSource { session(mode = "transcode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 200_000
        src.progressError = ReelError.Http(404, "Not found")
        c.pause()
        runCurrent()
        assertEquals(1, src.opens.size)
        src.progressError = null
        c.play()
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals(200.0, src.opens[1].startS, 0.0)
        // Reopened after a viewer pause, then play: autoplay.
        assertTrue(p.loads[1].autoplay)
    }

    @Test
    fun otherProgressErrorsAreIgnored() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        controller(src, p).start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 20_000
        src.progressError = ReelError.Unreachable()
        advanceTimeBy(30_001)
        assertEquals(1, src.opens.size)
    }

    @Test
    fun fatalErrorsRedecideOnceAMinute() = runTest {
        val src = FakeSource { session(mode = "transcode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 50_000
        p.listener!!.onFatalError("boom")
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals(50.0, src.opens[1].startS, 0.0)
        assertNull(c.state.value.error)

        // Again within the minute: give up with an error.
        advanceTimeBy(20_000)
        p.listener!!.onFatalError("boom")
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals("Playback stopped", c.state.value.error?.title)

        // Try again works; a minute later the automatic restart is allowed again.
        c.retry()
        runCurrent()
        assertEquals(3, src.opens.size)
        assertNull(c.state.value.error)
        advanceTimeBy(61_000)
        p.listener!!.onFatalError("boom")
        runCurrent()
        assertEquals(4, src.opens.size)
    }

    @Test
    fun decideErrorsAreShownWithTheSourcesWording() = runTest {
        val src = FakeSource { session() }
        src.openError = ReelError.RateLimited("Too many transcodes", null)
        val c = controller(src, FakePlayer())
        c.start(0.0)
        runCurrent()
        assertEquals("Reel is busy", c.state.value.error?.title)
        assertFalse(c.state.value.loading)
    }

    @Test
    fun audioSwitchesLocallyWhenThePlayerHasEveryTrack() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.audioTracks = 2
        c.selectAudio(2)
        assertEquals(1, p.audio)
        assertEquals(1, src.opens.size)
        assertEquals(2L, c.state.value.audioIndex)
    }

    @Test
    fun audioRedecidesWhenTheServerMustChangeIt() = runTest {
        val src = FakeSource { session(mode = "audio_transcode", audioAction = "encode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 90_000
        c.selectAudio(2)
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals(OpenRequest(90.0, audioIndex = 2, subtitleIndex = -1, quality = "auto"), src.opens[1])
    }

    @Test
    fun subtitles() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        // Sidecar VTT: local.
        c.selectSubtitle(3)
        assertEquals(TextChoice.Sidecar("reel-sub-3"), p.text)
        // Picture subtitle in direct play, and the player has it: local (embedded #1).
        p.embedded = 2
        c.selectSubtitle(4)
        assertEquals(TextChoice.Embedded(1), p.text)
        c.selectSubtitle(-1)
        assertEquals(TextChoice.Off, p.text)
        assertEquals(1, src.opens.size)
        // The player can't show it: the server burns it in.
        p.embedded = 0
        c.selectSubtitle(4)
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals(4L, src.opens[1].subtitleIndex)
    }

    @Test
    fun leavingBurnInRedecides() = runTest {
        val src = FakeSource { session(mode = "transcode", subAction = "burn_in", selectedSub = 4) }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        assertEquals(TextChoice.Off, p.text) // burned in: nothing to draw
        c.selectSubtitle(3)
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals(3L, src.opens[1].subtitleIndex)
    }

    @Test
    fun qualityRedecidesAtThePlayhead() = runTest {
        val src = FakeSource { session(mode = "transcode") }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(10.0)
        runCurrent()
        p.started()
        p.positionMs = 400_000
        c.selectQuality("720p")
        runCurrent()
        assertEquals(OpenRequest(400.0, audioIndex = 1, subtitleIndex = -1, quality = "720p"), src.opens[1])
        assertEquals(listOf("s1"), src.stops)
        assertEquals("720p", c.state.value.quality)
    }

    @Test
    fun endReportsAndPlayAfterEndStartsOver() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.started()
        p.positionMs = 3_600_000
        p.listener!!.onState(PlayerState.Ended)
        runCurrent()
        assertTrue(c.state.value.ended)
        assertEquals(ProgressRequest(3600.0, true), src.progress.last().second)
        c.play()
        assertEquals(0L, p.positionMs)
    }

    @Test
    fun aDecoderItLacksOffersATranscodeInsteadOfRetrying() = runTest {
        val src = FakeSource { session() }
        val p = FakePlayer()
        val c = controller(src, p)
        c.start(0.0)
        runCurrent()
        p.positionMs = 5_000
        p.listener!!.onFatalError("This device can't decode the stream", decoder = true)
        runCurrent()
        assertEquals(1, src.opens.size)
        val err = c.state.value.error!!
        assertFalse(err.retry)
        err.action!!.run()
        c.retry()
        runCurrent()
        assertEquals(2, src.opens.size)
        assertEquals("auto", src.opens[0].quality)
        assertEquals("720p", src.opens[1].quality)
    }
}

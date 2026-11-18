package app.reel.android.player

import app.reel.android.data.TuneResponse
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LiveControllerTest {
    private class Source : LiveSource {
        val calls = mutableListOf<String>()
        var tuneGate: CompletableDeferred<Unit>? = null
        var stopGate: CompletableDeferred<Unit>? = null
        var busy = false

        override suspend fun tune(channelId: Long, takeover: Boolean): TuneResponse {
            calls += "tune $channelId $takeover"
            tuneGate?.await()
            check(!busy || takeover)
            return TuneResponse("s$channelId", "passthrough", "/$channelId/stream.ts?sig=test", channelId, "Channel $channelId")
        }

        override suspend fun stop(sessionId: String) {
            calls += "stop $sessionId"
            stopGate?.await()
        }

        override fun errorFor(e: Throwable, takeover: () -> Unit) =
            PlayError("Busy", "Another viewer is watching.", action = PlayAction("Take over") { takeover() })
    }

    private fun TestScope.controller(source: Source, player: PlayerPort) =
        LiveController(source, player, backgroundScope, { "http://reel.test$it" }, { testScheduler.currentTime })

    @Test
    fun zappingOnlyTunesTheLastChannelAndWaitsForRelease() = runTest {
        val source = Source()
        val player = VodControllerTest.FakePlayer()
        val controller = controller(source, player)
        controller.tune(1)
        runCurrent()
        player.started()
        assertEquals(LivePhase.Playing, controller.state.value.phase)
        assertEquals(PlayItem.MIME_TS, player.loads.single().mimeType)
        assertEquals("http://reel.test/1/stream.ts?sig=test", player.loads.single().url)

        controller.zap(2)
        advanceTimeBy(100)
        controller.zap(3)
        assertEquals(3L, controller.state.value.channelId)
        source.stopGate = CompletableDeferred()
        advanceTimeBy(LiveController.ZAP_SETTLE_MS)
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1"), source.calls)
        source.stopGate!!.complete(Unit)
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1", "tune 3 false"), source.calls)
    }

    @Test
    fun anInFlightTuneIsReleasedBeforeItsReplacementStarts() = runTest {
        val source = Source().apply { tuneGate = CompletableDeferred() }
        val player = VodControllerTest.FakePlayer()
        val controller = controller(source, player)
        controller.tune(1)
        runCurrent()
        controller.tune(2)
        runCurrent()
        assertEquals(listOf("tune 1 false"), source.calls)
        source.tuneGate!!.complete(Unit)
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1", "tune 2 false"), source.calls)
        assertEquals("Channel 2", player.loads.single().title)
    }

    @Test
    fun leavingCleansUpAnInFlightTuneWithoutStartingPlayback() = runTest {
        val source = Source().apply { tuneGate = CompletableDeferred() }
        val player = VodControllerTest.FakePlayer()
        val controller = controller(source, player)
        controller.tune(1)
        runCurrent()
        controller.close()
        source.tuneGate!!.complete(Unit)
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1"), source.calls)
        assertEquals(emptyList<PlayItem>(), player.loads)
        assertNull(controller.state.value.session)
    }

    @Test
    fun backgroundReleasesAndForegroundRetunesThenCloseReleasesAgain() = runTest {
        val source = Source()
        val player = VodControllerTest.FakePlayer()
        val controller = controller(source, player)
        controller.tune(1)
        runCurrent()
        player.started()
        controller.background()
        runCurrent()
        assertEquals(LivePhase.Idle, controller.state.value.phase)
        assertNull(controller.state.value.session)
        controller.foreground()
        runCurrent()
        controller.close()
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1", "tune 1 false", "stop s1"), source.calls)
    }

    @Test
    fun takeoverAppliesToOneRetry() = runTest {
        val source = Source().apply { busy = true }
        val controller = controller(source, VodControllerTest.FakePlayer())
        controller.tune(1)
        runCurrent()
        controller.state.value.error!!.action!!.run()
        controller.retry()
        runCurrent()
        source.busy = false
        controller.tune(2)
        runCurrent()
        assertEquals(listOf("tune 1 false", "tune 1 true", "stop s1", "tune 2 false"), source.calls)
    }

    @Test
    fun repeatedStreamFailuresOnlyRetuneOncePerMinute() = runTest {
        val source = Source()
        val player = VodControllerTest.FakePlayer()
        val controller = controller(source, player)
        controller.tune(1)
        runCurrent()
        player.started()
        controller.onFatalError("Lost stream", decoder = false)
        runCurrent()
        player.started()
        controller.onFatalError("Lost stream", decoder = false)
        runCurrent()
        assertEquals(listOf("tune 1 false", "stop s1", "tune 1 false", "stop s1"), source.calls)
        assertEquals(LivePhase.Error, controller.state.value.phase)
    }
}

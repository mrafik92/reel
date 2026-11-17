package app.reel.android.ui.settings

import app.reel.android.data.Health
import app.reel.android.data.PlaybackPrefs
import app.reel.android.data.SessionInfo
import app.reel.android.ui.common.Load
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SettingsTest {
    private class Fake : SettingsBackend {
        var prefs = PlaybackPrefs()
        var failSave = false
        val saved = mutableListOf<PlaybackPrefs>()
        val revoked = mutableListOf<Long>()
        var list = listOf(SessionInfo(1, 100, current = true, client = "android"), SessionInfo(2, 50, client = "web"))

        override suspend fun preferences() = prefs

        override suspend fun savePreferences(p: PlaybackPrefs): PlaybackPrefs {
            if (failSave) throw IOException("down")
            saved += p
            prefs = p
            return p
        }

        override suspend fun sessions() = list

        override suspend fun revokeSession(id: Long) {
            revoked += id
        }

        override suspend fun health() = Health("ok", version = "0.9.3")
    }

    @Before
    fun setUp() = Dispatchers.setMain(UnconfinedTestDispatcher())

    @After
    fun tearDown() = Dispatchers.resetMain()

    @Test
    fun changesAreSavedToTheAccount() {
        val f = Fake()
        val vm = SettingsViewModel(f)
        vm.update { it.copy(subtitleMode = "always", subtitleLanguages = listOf("en", "fr")) }
        assertEquals(listOf(PlaybackPrefs(subtitleMode = "always", subtitleLanguages = listOf("en", "fr"))), f.saved)
        assertEquals(Load.Ready(f.prefs), vm.prefs.value)
        assertEquals("0.9.3", vm.server.value?.version)
    }

    @Test
    fun aFailedSaveRollsBackWithAMessage() {
        val f = Fake().apply { failSave = true }
        val vm = SettingsViewModel(f)
        vm.update { it.copy(preferSdh = true) }
        assertEquals(Load.Ready(PlaybackPrefs()), vm.prefs.value)
        assertNotNull(vm.message.value)
        f.failSave = false
        vm.update { it.copy(preferSdh = true) }
        assertNull(vm.message.value)
    }

    @Test
    fun signsOutOtherDevicesOnly() {
        val f = Fake()
        val vm = SettingsViewModel(f)
        val (me, web) = (vm.sessions.value as Load.Ready).value
        vm.revoke(me)
        vm.revoke(web)
        assertEquals(listOf(2L), f.revoked)
        assertEquals(listOf(1L), (vm.sessions.value as Load.Ready).value.map { it.id })
    }

    @Test
    fun languagesAndLabels() {
        assertEquals("English", SettingsLogic.languageName("en"))
        assertEquals("French", SettingsLogic.languageName("fre"))
        assertEquals("Original language", SettingsLogic.languageName("original"))
        assertEquals(listOf("b", "a", "c"), SettingsLogic.move(listOf("a", "b", "c"), 1, -1))
        assertEquals(listOf("a", "b", "c"), SettingsLogic.move(listOf("a", "b", "c"), 0, -1))
        assertEquals(listOf("b", "c", "a"), SettingsLogic.move(listOf("a", "b", "c"), 0, 5))
        val add = SettingsLogic.addable(listOf("original", "en"), audio = true)
        assertEquals("fr", add.first())
        assertEquals("original", SettingsLogic.addable(listOf("en"), audio = true).first())
        assertEquals("Automatic", SettingsLogic.subtitleMode("nonsense").label)
        assertEquals("Active now", SettingsLogic.lastSeen(1000, 1030))
        assertEquals("5 min ago", SettingsLogic.lastSeen(1000, 1300))
        assertEquals("3 h ago", SettingsLogic.lastSeen(0, 3 * 3600 + 10))
        assertEquals("4 days ago", SettingsLogic.lastSeen(0, 4 * 86_400))
        assertEquals("webOS", SettingsLogic.clientName("webos"))
    }
}

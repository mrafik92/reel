package app.reel.android.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.reel.android.data.Health
import app.reel.android.data.PlaybackPrefs
import app.reel.android.data.SessionInfo
import app.reel.android.ui.common.Load
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** The server calls Settings needs; the app passes `ReelApi` functions, tests pass fakes. */
interface SettingsBackend {
    suspend fun preferences(): PlaybackPrefs

    suspend fun savePreferences(p: PlaybackPrefs): PlaybackPrefs

    suspend fun sessions(): List<SessionInfo>

    suspend fun revokeSession(id: Long)

    suspend fun health(): Health
}

/**
 * Settings (#194): playback preferences (saved to the account on every change, rolled back with a
 * message if the save fails), signed-in devices (sign one out), and the server's version for About.
 */
class SettingsViewModel(private val backend: SettingsBackend) : ViewModel() {
    private val _prefs = MutableStateFlow<Load<PlaybackPrefs>>(Load.Loading)
    val prefs: StateFlow<Load<PlaybackPrefs>> = _prefs.asStateFlow()

    private val _sessions = MutableStateFlow<Load<List<SessionInfo>>>(Load.Loading)
    val sessions: StateFlow<Load<List<SessionInfo>>> = _sessions.asStateFlow()

    private val _server = MutableStateFlow<Health?>(null)
    val server: StateFlow<Health?> = _server.asStateFlow()

    /** The last failed action, shown once under its section. */
    private val _message = MutableStateFlow<String?>(null)
    val message: StateFlow<String?> = _message.asStateFlow()

    init {
        loadPrefs()
        loadSessions()
        viewModelScope.launch { _server.value = attempt { backend.health() } }
    }

    fun loadPrefs() = viewModelScope.launch {
        _prefs.value = Load.Loading
        _prefs.value = load { backend.preferences() }
    }

    fun loadSessions() = viewModelScope.launch {
        _sessions.value = Load.Loading
        _sessions.value = load { backend.sessions() }
    }

    /** Change the preferences: shown at once, saved, and put back if the server refuses. */
    fun update(change: (PlaybackPrefs) -> PlaybackPrefs) {
        val before = (_prefs.value as? Load.Ready)?.value ?: return
        val after = change(before)
        if (after == before) return
        _prefs.value = Load.Ready(after)
        _message.value = null
        viewModelScope.launch {
            try {
                val saved = backend.savePreferences(after)
                // A later change may already be showing; only adopt the answer for this one.
                if ((_prefs.value as? Load.Ready)?.value == after) _prefs.value = Load.Ready(saved)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if ((_prefs.value as? Load.Ready)?.value == after) _prefs.value = Load.Ready(before)
                _message.value = "Couldn't save: ${Load.Failed(e).message}"
            }
        }
    }

    /** Sign another device out. */
    fun revoke(s: SessionInfo) {
        if (s.current) return
        viewModelScope.launch {
            try {
                backend.revokeSession(s.id)
                val list = (_sessions.value as? Load.Ready)?.value.orEmpty()
                _sessions.value = Load.Ready(list.filter { it.id != s.id })
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _message.value = "Couldn't sign out ${s.deviceName ?: "that device"}: ${Load.Failed(e).message}"
            }
        }
    }

    private suspend fun <T> load(block: suspend () -> T): Load<T> = try {
        Load.Ready(block())
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Load.Failed(e)
    }

    private suspend fun <T> attempt(block: suspend () -> T): T? = (load(block) as? Load.Ready)?.value
}

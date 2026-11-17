package app.reel.android.data

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient

/** Where the app is in the connect → sign-in flow. The UI renders one screen per state. */
sealed interface AuthState {
    data object Loading : AuthState

    /** No server yet. [suggested] prefills the address field (last server or `REEL_URL`). */
    data class NeedsServer(val suggested: String = "") : AuthState

    /** A server, no session. [notice] explains why (e.g. the session ended). */
    data class NeedsSignIn(val server: ServerAddress, val notice: String? = null) : AuthState

    data class SignedIn(val server: ServerAddress, val user: Me) : AuthState
}

/**
 * Owns the session: the stored server and token, the current [ReelApi], and the [AuthState]. One
 * per process (see `AppGraph`). Screens call [api] for data; a 401 anywhere lands back on sign-in.
 */
class SessionManager(
    private val store: SessionStore,
    private val http: OkHttpClient,
    private val scope: CoroutineScope,
    private val deviceName: String,
    private val defaultServer: String = "",
    private val isOnline: () -> Boolean = { true },
) {
    private val _state = MutableStateFlow<AuthState>(AuthState.Loading)
    val state: StateFlow<AuthState> = _state.asStateFlow()

    @Volatile private var token: String? = null

    @Volatile private var current: ReelApi? = null

    /** The API of the configured server. Only `null` before a server is picked. */
    val api: ReelApi? get() = current

    /** Read the stored session. Signed in users go straight in (offline too); `/me` refreshes in the background. */
    suspend fun start() {
        val s = store.current()
        val server = s.server
        token = s.token
        current = server?.let(::apiFor)
        _state.value = when {
            server == null -> AuthState.NeedsServer(defaultServer)
            s.token == null || s.user == null -> AuthState.NeedsSignIn(server)
            else -> AuthState.SignedIn(server, s.user)
        }
        if (_state.value is AuthState.SignedIn) scope.launch { refreshUser() }
    }

    /** Re-read `/me` (permissions may have changed). Network errors are ignored; a 401 signs out. */
    suspend fun refreshUser() {
        val api = current ?: return
        try {
            val me = api.me()
            store.updateUser(me)
            val st = _state.value
            if (st is AuthState.SignedIn) _state.value = st.copy(user = me)
        } catch (_: ReelError) {
            // Unauthorized already went through onUnauthorized.
        }
    }

    /**
     * Check that [input] is a Reel server that's ready for sign-in, then remember it. Throws
     * [ReelError] (or [IllegalArgumentException] for an address that can't be parsed).
     */
    suspend fun connect(input: String): ServerAddress {
        val server = ServerAddress.parse(input) ?: throw IllegalArgumentException("Enter a server address, like 192.168.1.5:8420")
        val probe = apiFor(server)
        val health = try {
            probe.health()
        } catch (e: ReelError.Http) {
            throw ReelError.Http(e.status, "That address doesn't look like a Reel server (HTTP ${e.status})")
        } catch (e: ReelError.BadResponse) {
            throw ReelError.Http(0, "That address doesn't look like a Reel server")
        }
        if (health.status != "ok") throw ReelError.Http(0, "The server isn't healthy (${health.status})")
        if (probe.setupStatus().needsSetup) {
            throw ReelError.Http(0, "This server isn't set up yet. Finish setup in a web browser first.")
        }
        store.setServer(server)
        val s = store.current()
        token = s.token
        current = probe
        _state.value = if (s.token != null && s.user != null) AuthState.SignedIn(server, s.user) else AuthState.NeedsSignIn(server)
        return server
    }

    /** Go back to the address screen (keeps the session until another server is picked). */
    fun changeServer() {
        _state.value = AuthState.NeedsServer(current?.server?.origin ?: defaultServer)
    }

    /** Throws [ReelError]; a wrong password is [ReelError.Http] 401, a lockout [ReelError.RateLimited]. */
    suspend fun signIn(username: String, password: String) {
        val api = current ?: error("no server")
        val res = api.login(
            LoginBody(
                username = username.trim(),
                password = password,
                deviceId = store.deviceId(),
                deviceName = deviceName,
            ),
        )
        store.signedIn(res.token, res.user)
        token = res.token
        _state.value = AuthState.SignedIn(api.server, res.user)
    }

    /** Revoke the session on the server (best effort) and forget it here. */
    suspend fun signOut() {
        val api = current
        if (token != null && api != null) {
            try {
                api.logout()
            } catch (_: ReelError) {
                // Offline or already revoked: the local sign-out still happens.
            }
        }
        dropSession(null)
    }

    private suspend fun dropSession(notice: String?) {
        token = null
        store.clearToken()
        val server = current?.server
        _state.value = if (server != null) AuthState.NeedsSignIn(server, notice) else AuthState.NeedsServer(defaultServer)
    }

    private fun apiFor(server: ServerAddress): ReelApi {
        var self: ReelApi? = null
        return ReelApi(
            server = server,
            base = http,
            token = { token },
            isOnline = isOnline,
            onUnauthorized = {
                // Only the live API may end the session (not a probe or a previous server's).
                if (token != null && current === self) {
                    scope.launch { dropSession("Your session has ended. Please sign in again.") }
                }
            },
        ).also { self = it }
    }
}

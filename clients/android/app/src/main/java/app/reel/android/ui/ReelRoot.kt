package app.reel.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.reel.android.LocalAppGraph
import app.reel.android.data.AuthState
import app.reel.android.ui.auth.ServerScreen
import app.reel.android.ui.auth.SignInScreen
import app.reel.android.ui.shell.AppShell
import app.reel.android.ui.theme.ReelColors

/** Root of the app: one screen per [AuthState]. */
@Composable
fun ReelRoot() {
    val session = LocalAppGraph.current.session
    val state by session.state.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { if (session.state.value == AuthState.Loading) session.start() }
    when (val s = state) {
        AuthState.Loading -> Box(Modifier.fillMaxSize().background(ReelColors.Bg), contentAlignment = Alignment.Center) {
            CircularProgressIndicator()
        }
        is AuthState.NeedsServer -> ServerScreen(s.suggested)
        is AuthState.NeedsSignIn -> SignInScreen(s.server, s.notice)
        // A new server or user starts a fresh shell (navigation state belongs to one session).
        is AuthState.SignedIn -> key(s.server, s.user.id) { AppShell(s.user) }
    }
}

package app.reel.android.ui.auth

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import app.reel.android.LocalAppGraph
import app.reel.android.R
import app.reel.android.data.Me
import app.reel.android.data.ReelError
import app.reel.android.data.ServerAddress
import app.reel.android.ui.theme.ReelColors
import kotlinx.coroutines.launch

/** What a sign-in screen shows for a failure. */
internal fun signInMessage(e: Throwable): String = when (e) {
    is ReelError.Http -> if (e.status == 401) "Wrong username or password." else e.userMessage
    is ReelError -> e.userMessage
    is IllegalArgumentException -> e.message ?: "Invalid address"
    else -> "Something went wrong"
}

/** Logo, title and a narrow centered column; scrolls when the keyboard is up. */
@Composable
private fun AuthScaffold(title: String, subtitle: String?, content: @Composable ColumnScope.() -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .background(ReelColors.Bg)
            .safeDrawingPadding()
            .imePadding(),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .widthIn(max = 440.dp)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Image(painterResource(R.drawable.logo), contentDescription = null, modifier = Modifier.size(88.dp))
            Text(title, style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
            if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
            content()
        }
    }
}

@Composable
private fun ErrorText(msg: String?) {
    if (msg != null) {
        Text(
            msg,
            style = MaterialTheme.typography.bodyMedium,
            color = ReelColors.AccentHi,
            modifier = Modifier.fillMaxWidth().testTag("auth-error"),
        )
    }
}

@Composable
private fun SubmitButton(label: String, busy: Boolean, onClick: () -> Unit) {
    Button(onClick = onClick, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
        if (busy) {
            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = ReelColors.AccentInk)
        } else {
            Text(label)
        }
    }
}

/** Step 1: type the server address once (LAN, Tailscale or a public name). */
@Composable
fun ServerScreen(suggested: String) {
    val session = LocalAppGraph.current.session
    val scope = rememberCoroutineScope()
    var address by rememberSaveable { mutableStateOf(suggested) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }

    fun submit() {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                session.connect(address)
            } catch (e: Exception) {
                error = signInMessage(e)
            } finally {
                busy = false
            }
        }
    }

    AuthScaffold("Connect to Reel", "Your server's address, as you'd open it in a browser.") {
        OutlinedTextField(
            value = address,
            onValueChange = { address = it },
            label = { Text("Server address") },
            placeholder = { Text("192.168.1.5:8420") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onGo = { submit() }),
            modifier = Modifier.fillMaxWidth().focusRequester(focus).testTag("server-address"),
        )
        ErrorText(error)
        SubmitButton("Connect", busy, ::submit)
    }
}

/** Step 2: username and password. */
@Composable
fun SignInScreen(server: ServerAddress, notice: String?) {
    val session = LocalAppGraph.current.session
    val scope = rememberCoroutineScope()
    var username by rememberSaveable { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var error by remember(notice) { mutableStateOf(notice) }
    var busy by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    val passwordFocus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }

    fun submit() {
        if (busy || username.isBlank() || password.isEmpty()) return
        busy = true
        error = null
        scope.launch {
            try {
                session.signIn(username, password)
            } catch (e: Exception) {
                error = signInMessage(e)
                password = ""
            } finally {
                busy = false
            }
        }
    }

    AuthScaffold("Sign in", server.origin) {
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text("Username") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onNext = { passwordFocus.requestFocus() }),
            modifier = Modifier.fillMaxWidth().focusRequester(focus).testTag("username"),
        )
        OutlinedTextField(
            value = password,
            onValueChange = { password = it },
            label = { Text("Password") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { submit() }),
            modifier = Modifier.fillMaxWidth().focusRequester(passwordFocus).testTag("password"),
        )
        ErrorText(error)
        SubmitButton("Sign in", busy, ::submit)
        TextButton(onClick = { session.changeServer() }) { Text("Change server") }
    }
}

/** Signed in. The app shell (#187) replaces this with Home and the navigation. */
@Composable
fun SignedInPlaceholder(user: Me) {
    val session = LocalAppGraph.current.session
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    AuthScaffold("Signed in as ${user.username}", session.api?.server?.origin) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = { scope.launch { session.signOut() } }, modifier = Modifier.focusRequester(focus)) {
                Text("Sign out")
            }
        }
    }
}

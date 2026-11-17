package app.reel.android.ui.common

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ReelApi
import app.reel.android.data.ReelError
import app.reel.android.ui.theme.ReelColors
import kotlinx.coroutines.CancellationException

/** One screen read: loading, failed, or ready. */
sealed interface Load<out T> {
    data object Loading : Load<Nothing>

    data class Failed(val error: Throwable) : Load<Nothing> {
        val message: String get() = (error as? ReelError)?.userMessage ?: "Something went wrong"
    }

    data class Ready<T>(val value: T) : Load<T>
}

/** A screen read with [retry]. Made by [rememberLoad]. */
@Stable
class LoadHandle<T> internal constructor() {
    var state: Load<T> by mutableStateOf(Load.Loading)
        internal set
    internal var attempt by mutableIntStateOf(0)

    fun retry() {
        attempt++
    }
}

/**
 * Run [block] against the server's API when the screen opens (and again when [keys] change or
 * [LoadHandle.retry] is called). A 401 inside signs out by itself (see `SessionManager`).
 *
 * ```
 * val home = rememberLoad { get<Home>("/home") }
 * Loadable(home, isEmpty = { it.rows.isEmpty() }) { h -> HomeRows(h) }
 * ```
 */
@Composable
fun <T> rememberLoad(vararg keys: Any?, block: suspend ReelApi.() -> T): LoadHandle<T> {
    val api = LocalAppGraph.current.session.api
    val handle = remember(*keys) { LoadHandle<T>() }
    LaunchedEffect(handle, handle.attempt, api) {
        if (api == null) return@LaunchedEffect
        handle.state = Load.Loading
        handle.state = try {
            Load.Ready(api.block())
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Load.Failed(e)
        }
    }
    return handle
}

/** Renders [handle]: a spinner, an error with *Try again*, [emptyText], or [content]. */
@Composable
fun <T> Loadable(
    handle: LoadHandle<T>,
    modifier: Modifier = Modifier,
    isEmpty: (T) -> Boolean = { false },
    emptyText: String = "Nothing here yet.",
    content: @Composable (T) -> Unit,
) {
    when (val s = handle.state) {
        Load.Loading -> LoadingView(modifier)
        is Load.Failed -> ErrorView(s.message, onRetry = handle::retry, modifier = modifier)
        is Load.Ready -> if (isEmpty(s.value)) EmptyView(emptyText, modifier) else content(s.value)
    }
}

@Composable
fun LoadingView(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize().testTag("loading"), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = ReelColors.Accent)
    }
}

/** A failure with *Try again*. On TV the button takes focus, so OK retries. */
@Composable
fun ErrorView(message: String, onRetry: (() -> Unit)?, modifier: Modifier = Modifier) {
    val focus = remember { FocusRequester() }
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    CenteredMessage(message, modifier.testTag("error")) {
        if (onRetry != null) {
            Button(onClick = onRetry, modifier = Modifier.focusRequester(focus).testTag("retry")) { Text("Try again") }
            if (tv) LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
        }
    }
}

@Composable
fun EmptyView(message: String, modifier: Modifier = Modifier) = CenteredMessage(message, modifier.testTag("empty")) {}

@Composable
private fun CenteredMessage(message: String, modifier: Modifier, actions: @Composable () -> Unit) {
    Box(modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(
            Modifier.widthIn(max = 480.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(message, style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2, textAlign = TextAlign.Center)
            actions()
        }
    }
}

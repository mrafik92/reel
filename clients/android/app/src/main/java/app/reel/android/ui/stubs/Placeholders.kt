package app.reel.android.ui.stubs

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import app.reel.android.BuildConfig
import app.reel.android.LocalAppGraph
import app.reel.android.data.ItemDetail
import app.reel.android.data.Me
import app.reel.android.player.resumePositionS
import app.reel.android.ui.common.Loadable
import app.reel.android.ui.common.rememberLoad
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.vod.VodRoutes
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.launch

// Stand-ins registered in ReelNavGraph until their tickets replace them. Keep them dumb.

/** Replaced by #189. */
@Composable
fun SearchPlaceholder() = EmptyView("Search is coming in a later update.")

/** Replaced by #191. */
@Composable
fun LivePlaceholder() = EmptyView("Live TV is coming in a later update.")

/** Replaced by #188 (libraries grid); keep the IPTV entry (#193) for users with VOD access. */
@Composable
fun LibraryPlaceholder(user: Me) {
    if (!user.canWatchVod) return EmptyView("Your libraries are coming in a later update.")
    val nav = LocalNavController.current
    val focus = remember { FocusRequester() }
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Your libraries are coming in a later update.", style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2)
        Button(onClick = { nav.navigate(VodRoutes.browse("movie")) }, modifier = Modifier.focusRequester(focus).testTag("open-vod")) {
            Text("IPTV movies & series")
        }
        LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    }
}

/** Replaced by #188 (item detail). Until then it can start the player (#190). */
@Composable
fun ItemPlaceholder(id: Long) {
    val item = rememberLoad(id) { get<ItemDetail>("/items/$id") }
    val nav = LocalNavController.current
    val focus = remember { FocusRequester() }
    Loadable(item) { d ->
        Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(d.title, style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
            Text("Full details are coming in a later update.", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
            val file = d.files.firstOrNull()
            if (d.playable && file != null) {
                val resume = resumePositionS(d)
                Button(
                    onClick = { nav.navigate(Routes.player(file.id, d.id, resume)) },
                    modifier = Modifier.focusRequester(focus).testTag("play"),
                ) { Text(if (resume > 0) "Resume" else "Play") }
                if (resume > 0) {
                    Button(onClick = { nav.navigate(Routes.player(file.id, d.id, 0.0)) }, modifier = Modifier.testTag("play-start")) {
                        Text("Play from the start")
                    }
                }
                LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
            }
        }
    }
}

/** Replaced by #194; keeps *Sign out* reachable meanwhile. */
@Composable
fun SettingsPlaceholder(user: Me) {
    val session = LocalAppGraph.current.session
    val scope = rememberCoroutineScope()
    val memory = rememberFocusMemory()
    RestoreFocus(memory)
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Settings", style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
        Text("Signed in as ${user.username}", style = MaterialTheme.typography.bodyLarge, color = ReelColors.Ink2)
        Text(session.api?.server?.origin.orEmpty(), style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        Text("Reel for Android ${BuildConfig.VERSION_NAME}", style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3)
        Button(
            onClick = { scope.launch { session.signOut() } },
            modifier = Modifier.focusMemory(memory, "sign-out").testTag("sign-out"),
        ) { Text("Sign out") }
    }
}

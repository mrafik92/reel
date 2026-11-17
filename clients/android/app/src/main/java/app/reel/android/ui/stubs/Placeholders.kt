package app.reel.android.ui.stubs

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import app.reel.android.BuildConfig
import app.reel.android.LocalAppGraph
import app.reel.android.data.Me
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.item.ItemLogic
import app.reel.android.ui.shell.PlayerArgs
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

/** Replaced by #190 (VOD player). */
@Composable
fun PlayerPlaceholder(args: PlayerArgs) = EmptyView(
    "The player is coming in a later update (file ${args.fileId}, item ${args.itemId}, from " +
        "${ItemLogic.clock(args.startS)}). Press Back to return.",
)

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

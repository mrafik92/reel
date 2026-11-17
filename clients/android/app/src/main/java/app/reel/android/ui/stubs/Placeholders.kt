package app.reel.android.ui.stubs

import androidx.compose.runtime.Composable
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.item.ItemLogic
import app.reel.android.ui.shell.PlayerArgs

// Stand-ins registered in ReelNavGraph until their tickets replace them. Keep them dumb.

/** Replaced by #191. */
@Composable
fun LivePlaceholder() = EmptyView("Live TV is coming in a later update.")

/** Replaced by #190 (VOD player). */
@Composable
fun PlayerPlaceholder(args: PlayerArgs) = EmptyView(
    "The player is coming in a later update (file ${args.fileId}, item ${args.itemId}, from " +
        "${ItemLogic.clock(args.startS)}). Press Back to return.",
)

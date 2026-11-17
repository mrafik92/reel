package app.reel.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard

/**
 * A rounded button for actions, tabs and filters (Play, a season, *Unwatched*). [selected] fills it
 * with the accent wash; focus (TV) inverts it to cream like the nav rail. [primary] is the main
 * action: cream, turning to the bright accent when focused so it still stands out.
 */
@Composable
fun Pill(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    selected: Boolean = false,
    primary: Boolean = false,
    enabled: Boolean = true,
) {
    FocusCard(onClick = { if (enabled) onClick() }, modifier = modifier, shape = RoundedCornerShape(50)) { focused ->
        val bg = when {
            primary && focused -> ReelColors.AccentHi
            focused || primary -> ReelColors.Ink
            selected -> ReelColors.AccentWash
            else -> ReelColors.Bg2
        }
        val fg = when {
            primary && focused -> ReelColors.AccentInk
            focused || primary -> ReelColors.Bg
            selected -> ReelColors.AccentHi
            !enabled -> ReelColors.Ink3
            else -> ReelColors.Ink
        }
        // Fill the whole pill (it may be given a fixed width), with the label centred in it.
        Box(Modifier.matchParentSize().background(bg))
        Row(
            Modifier.align(Alignment.Center).heightIn(min = 40.dp).padding(horizontal = 18.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (icon != null) Icon(icon, contentDescription = null, tint = fg, modifier = Modifier.size(20.dp))
            Text(text, style = MaterialTheme.typography.labelLarge, color = fg, maxLines = 1)
        }
    }
}

/**
 * A list of choices in a dialog (sort order, genre). The current choice takes focus when it opens,
 * so on TV OK confirms it and Back closes the dialog without a change.
 */
@Composable
fun <T> ChoiceDialog(
    title: String,
    options: List<T>,
    selected: T,
    label: (T) -> String,
    onPick: (T) -> Unit,
    onDismiss: () -> Unit,
) {
    val start = remember { FocusRequester() }
    Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier
                .widthIn(max = 420.dp)
                .background(ReelColors.Bg1, RoundedCornerShape(16.dp))
                .padding(vertical = 16.dp)
                .testTag("choice-dialog"),
        ) {
            Text(title, style = MaterialTheme.typography.titleMedium, color = ReelColors.Ink, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
            LazyColumn(Modifier.heightIn(max = 480.dp)) {
                itemsIndexed(options) { _, o ->
                    val isSel = o == selected
                    FocusCard(
                        onClick = { onPick(o) },
                        shape = RoundedCornerShape(0.dp),
                        modifier = Modifier.fillMaxWidth().then(if (isSel) Modifier.focusRequester(start) else Modifier).testTag("choice-${label(o)}"),
                    ) { focused ->
                        Row(
                            Modifier.fillMaxWidth().background(if (focused) ReelColors.Ink else ReelColors.Bg1).padding(horizontal = 24.dp, vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(label(o), style = MaterialTheme.typography.bodyLarge, color = if (focused) ReelColors.Bg else ReelColors.Ink, modifier = Modifier.weight(1f))
                            if (isSel) Icon(Icons.Outlined.Check, contentDescription = "Selected", tint = if (focused) ReelColors.Bg else ReelColors.AccentHi)
                        }
                    }
                }
            }
        }
        LaunchedEffect(Unit) {
            repeat(5) {
                kotlinx.coroutines.delay(16)
                if (runCatching { start.requestFocus() }.isSuccess) return@LaunchedEffect
            }
        }
    }
}

/** A centred, padded footer under a list or grid (loading more, end of list). */
@Composable
fun FooterBox(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { content() }
}

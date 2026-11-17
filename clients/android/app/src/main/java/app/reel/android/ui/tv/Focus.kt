package app.reel.android.ui.tv

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.theme.ReelColors

/**
 * The D-pad focus kit (#187). Every screen uses these instead of inventing its own focus handling:
 *
 * - [FocusCard]: a clickable surface that scales up with a cream outline when focused.
 * - [FocusMemory] + [Modifier.focusMemory] + [RestoreFocus]: on TV, a screen focuses its first
 *   element on entry, and the element it left from when the user comes back (Back from a detail
 *   page). The remembered key survives navigation (it's saveable).
 * - Rows keep their scroll position by using `rememberLazyListState()` inside keyed lazy items.
 */

/** Scale of a focused card (TV look; on touch nothing is focused, so phones never see it). */
const val FOCUS_SCALE = 1.08f

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun FocusCard(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(8.dp),
    onLongClick: (() -> Unit)? = null,
    content: @Composable BoxScope.(focused: Boolean) -> Unit,
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    val scale by animateFloatAsState(if (focused) FOCUS_SCALE else 1f, label = "focus-scale")
    Box(
        modifier
            .zIndex(if (focused) 1f else 0f)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .border(if (focused) 3.dp else 0.dp, if (focused) ReelColors.Ink else Color.Transparent, shape)
            .clip(shape)
            .combinedClickable(interactionSource = source, indication = ripple(), onClick = onClick, onLongClick = onLongClick),
    ) { content(focused) }
}

/**
 * Which element of a screen had focus last, by a stable key (e.g. `"row:3/item:42"`). Create it
 * with [rememberFocusMemory], tag elements with [Modifier.focusMemory], and call [RestoreFocus].
 */
@Stable
class FocusMemory internal constructor(initial: String?) {
    var lastKey: String? = initial
        private set
    private val requesters = LinkedHashMap<String, FocusRequester>()

    internal fun requester(key: String): FocusRequester = requesters.getOrPut(key) { FocusRequester() }

    internal fun onFocused(key: String) {
        lastKey = key
    }

    /** Focus the remembered element, else the first tagged one. False if neither is on screen yet. */
    fun restore(): Boolean {
        val keys = listOfNotNull(lastKey) + requesters.keys.take(1)
        for (k in keys) {
            val r = requesters[k] ?: continue
            if (runCatching { r.requestFocus() }.isSuccess) return true
        }
        return false
    }

    companion object {
        val Saver: Saver<FocusMemory, String> = Saver(save = { it.lastKey ?: "" }, restore = { FocusMemory(it.ifEmpty { null }) })
    }
}

@Composable
fun rememberFocusMemory(): FocusMemory = rememberSaveable(saver = FocusMemory.Saver) { FocusMemory(null) }

/** Tag a focusable element, or a container of one (a card), so [memory] can restore it. */
fun Modifier.focusMemory(memory: FocusMemory, key: String): Modifier =
    focusRequester(memory.requester(key)).onFocusChanged { if (it.hasFocus) memory.onFocused(key) }

/**
 * On TV, focus the screen's remembered (or first) element once it's on screen. Phones skip it:
 * touch has no focus, and a focused card would show its outline.
 *
 * [ready] is false while the content is still loading; restoring waits for it.
 */
@Composable
fun RestoreFocus(memory: FocusMemory, ready: Boolean = true) {
    if (LocalDeviceKind.current != DeviceKind.Tv) return
    LaunchedEffect(ready) {
        if (!ready) return@LaunchedEffect
        // Lazy rows compose their items a frame or two after the screen; try for a few frames.
        repeat(10) {
            withFrameNanos { }
            if (memory.restore()) return@LaunchedEffect
        }
    }
}

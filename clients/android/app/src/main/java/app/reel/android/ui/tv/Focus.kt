package app.reel.android.ui.tv

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.gestures.BringIntoViewSpec
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
 * - [FocusCard]: a clickable surface that scales up (×1.1) with a 3dp cream outline and a shadow
 *   when focused.
 * - [FocusMemory] + [Modifier.focusMemory] + [RestoreFocus]: on TV, a screen focuses its first
 *   element on entry, and the element it left from when the user comes back (Back from a detail
 *   page). The remembered key survives navigation (it's saveable).
 * - Rows keep their scroll position by using `rememberLazyListState()` inside keyed lazy items.
 */

/** Scale of a focused card (TV look; on touch nothing is focused, so phones never see it). */
const val FOCUS_SCALE = 1.1f

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun FocusCard(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(8.dp),
    onLongClick: (() -> Unit)? = null,
    /** Wide elements (an episode row) scale less, or they'd spill over the screen edge. */
    focusScale: Float = FOCUS_SCALE,
    content: @Composable BoxScope.(focused: Boolean) -> Unit,
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    val scale by animateFloatAsState(if (focused) focusScale else 1f, label = "focus-scale")
    val cardShape = shape
    Box(
        modifier
            .zIndex(if (focused) 1f else 0f)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                // A soft drop shadow lifts the focused card off the row.
                shadowElevation = if (focused) 16.dp.toPx() else 0f
                this.shape = cardShape
                clip = false
            }
            .border(if (focused) 3.dp else 0.dp, if (focused) ReelColors.Ink else Color.Transparent, shape)
            .clip(shape)
            .combinedClickable(interactionSource = source, indication = ripple(), onClick = onClick, onLongClick = onLongClick),
    ) { content(focused) }
}

/** Scale for full-width focusable rows (episode lists, settings rows). */
const val ROW_FOCUS_SCALE = 1.02f

/**
 * Scroll only as far as needed to show the focused element. On TV, Compose's default scrolls lazy
 * lists so focus sits about a third of the way down (good for rows of cards), which pushes a detail
 * page's title off the top when its Play button takes focus. Provide it with
 * `CompositionLocalProvider(LocalBringIntoViewSpec provides KeepInView)`.
 */
@OptIn(ExperimentalFoundationApi::class)
val KeepInView: BringIntoViewSpec = object : BringIntoViewSpec {
    override fun calculateScrollDistance(offset: Float, size: Float, containerSize: Float): Float {
        val end = offset + size
        return when {
            offset >= 0 && end <= containerSize -> 0f
            offset < 0 || size > containerSize -> offset
            else -> end - containerSize
        }
    }
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

    /**
     * Focus the remembered element, else [first] (when given and on screen), else the first tagged
     * one. False if none of them is on screen yet.
     */
    fun restore(first: String? = null): Boolean {
        val keys = listOfNotNull(lastKey, first) + requesters.keys.take(1)
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
 * [ready] is false while the content is still loading; restoring waits for it. [first] names the
 * element to start on when nothing is remembered (e.g. Play), if it isn't the first one tagged.
 */
@Composable
fun RestoreFocus(memory: FocusMemory, ready: Boolean = true, first: String? = null) {
    if (LocalDeviceKind.current != DeviceKind.Tv) return
    LaunchedEffect(ready) {
        if (!ready) return@LaunchedEffect
        // Lazy rows compose their items a frame or two after the screen; try for a few frames.
        repeat(10) {
            withFrameNanos { }
            if (memory.restore(first)) return@LaunchedEffect
        }
    }
}

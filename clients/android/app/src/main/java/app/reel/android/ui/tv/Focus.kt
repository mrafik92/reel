package app.reel.android.ui.tv

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.gestures.BringIntoViewSpec
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.theme.ReelDimensions

/**
 * The D-pad focus kit (#187). Every screen uses these instead of inventing its own focus handling:
 *
 * - [FocusCard]: a clickable surface with an inset, two-tone focus outline.
 * - [FocusMemory] + [Modifier.focusMemory] + [RestoreFocus]: on TV, a screen focuses its first
 *   element on entry, and the element it left from when the user comes back (Back from a detail
 *   page). The remembered key survives navigation (it's saveable).
 * - Rows keep their scroll position by using `rememberLazyListState()` inside keyed lazy items.
 */

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun FocusCard(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(ReelDimensions.CardRadius),
    onLongClick: (() -> Unit)? = null,
    enabled: Boolean = true,
    content: @Composable BoxScope.(focused: Boolean) -> Unit,
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    Box(
        modifier
            .clip(shape)
            .combinedClickable(
                interactionSource = source,
                indication = ripple(),
                enabled = enabled,
                onClick = onClick,
                onLongClick = onLongClick,
            ),
    ) {
        content(focused)
        if (focused) {
            Box(
                Modifier.matchParentSize()
                    .border(ReelDimensions.FocusOutline, ReelColors.Bg, shape)
                    .padding(ReelDimensions.FocusOutline)
                    .border(ReelDimensions.FocusOutline, ReelColors.Ink, shape),
            )
        }
    }
}

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

    private var focusedKey: String? = null
    private var previousOrder: List<String> = emptyList()

    internal fun attach(key: String, requester: FocusRequester) {
        requesters[key] = requester
    }

    internal fun detach(key: String, requester: FocusRequester) {
        if (requesters.remove(key, requester) && focusedKey == key) focusedKey = null
    }

    internal fun onFocusChanged(key: String, focused: Boolean) {
        if (focused) {
            focusedKey = key
            lastKey = key
            previousOrder = requesters.keys.toList()
        } else if (focusedKey == key) {
            focusedKey = null
        }
    }

    /** Try the saved key, another attached item in its group, the preferred entry, then any target. */
    fun restore(first: String? = null): Boolean {
        val saved = lastKey
        val group = saved?.substringBeforeLast("/", missingDelimiterValue = "")?.takeIf { it.isNotEmpty() }
        val sameGroup = if (group == null) emptyList() else requesters.keys.filter {
            it.substringBeforeLast("/", missingDelimiterValue = "") == group
        }
        val savedIndex = previousOrder.indexOf(saved)
        val nearby = if (savedIndex < 0) sameGroup else sameGroup.sortedBy { key ->
            val index = previousOrder.indexOf(key)
            if (index < 0) Int.MAX_VALUE else kotlin.math.abs(index - savedIndex)
        }
        val candidates = (listOfNotNull(saved) + nearby + listOfNotNull(first) + requesters.keys).distinct()
        for (key in candidates) {
            val requester = requesters[key] ?: continue
            // Compose 1.7's requestFocus returns Unit, including when the target declines focus.
            if (runCatching { requester.requestFocus() }.isSuccess && focusedKey == key) return true
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
fun Modifier.focusMemory(memory: FocusMemory, key: String): Modifier = composed {
    val requester = remember(memory, key) { FocusRequester() }
    DisposableEffect(memory, key, requester) {
        memory.attach(key, requester)
        onDispose { memory.detach(key, requester) }
    }
    focusRequester(requester).onFocusChanged { memory.onFocusChanged(key, it.hasFocus) }
}

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
    LaunchedEffect(memory, ready, first) {
        if (!ready) return@LaunchedEffect
        // Lazy rows compose their items a frame or two after the screen; try for a few frames.
        repeat(10) {
            withFrameNanos { }
            if (memory.restore(first)) return@LaunchedEffect
        }
    }
}

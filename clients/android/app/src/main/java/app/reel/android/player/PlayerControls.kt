package app.reel.android.player

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard

val Scrim = Color(0xCC000000)
val TopShade = Brush.verticalGradient(listOf(Color(0xCC000000), Color.Transparent))
val BottomShade = Brush.verticalGradient(listOf(Color.Transparent, Color(0xE6000000)))

/** "1:02:03" / "4:05". */
fun formatTime(ms: Long): String {
    val s = (ms.coerceAtLeast(0) / 1000)
    val h = s / 3600
    val m = (s % 3600) / 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}

/**
 * The timeline. Touch: tap or drag to seek. D-pad: left/right seek ±10 s while it has focus
 * (the cream outline shows it).
 */
@Composable
fun SeekBar(
    positionMs: Long,
    durationMs: Long?,
    bufferedMs: Long,
    onSeek: (Long) -> Unit,
    onNudge: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val source = remember { MutableInteractionSource() }
    val focused by source.collectIsFocusedAsState()
    var scrub by remember { mutableStateOf<Float?>(null) }
    val dur = durationMs ?: 0L
    val frac = scrub ?: if (dur > 0) (positionMs.toFloat() / dur).coerceIn(0f, 1f) else 0f
    val buf = if (dur > 0) (bufferedMs.toFloat() / dur).coerceIn(0f, 1f) else 0f
    Box(
        modifier
            .height(32.dp)
            .semantics { contentDescription = "Seek bar" }
            .onKeyEvent { e ->
                if (e.type != KeyEventType.KeyDown) return@onKeyEvent false
                when (e.key) {
                    Key.DirectionLeft -> { onNudge(-PlayerKeys.SEEK_S); true }
                    Key.DirectionRight -> { onNudge(PlayerKeys.SEEK_S); true }
                    else -> false
                }
            }
            .focusable(interactionSource = source)
            .pointerInput(dur) {
                detectTapGestures { o -> if (dur > 0) onSeek((o.x / size.width * dur).toLong()) }
            }
            .pointerInput(dur) {
                detectHorizontalDragGestures(
                    onDragStart = { o -> scrub = (o.x / size.width).coerceIn(0f, 1f) },
                    onDragEnd = {
                        scrub?.let { if (dur > 0) onSeek((it * dur).toLong()) }
                        scrub = null
                    },
                    onDragCancel = { scrub = null },
                    onHorizontalDrag = { change, _ -> scrub = (change.position.x / size.width).coerceIn(0f, 1f) },
                )
            }
            .testTag("seek-bar"),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.fillMaxWidth().height(32.dp)) {
            val h = if (focused) 8.dp.toPx() else 5.dp.toPx()
            val y = (size.height - h) / 2
            val r = CornerRadius(h / 2, h / 2)
            drawRoundRect(Color(0x55FFFFFF), Offset(0f, y), Size(size.width, h), r)
            drawRoundRect(Color(0x88FFFFFF), Offset(0f, y), Size(size.width * buf, h), r)
            drawRoundRect(ReelColors.Accent, Offset(0f, y), Size(size.width * frac, h), r)
            val knob = if (focused) 11.dp.toPx() else 7.dp.toPx()
            drawCircle(if (focused) ReelColors.Ink else ReelColors.AccentHi, knob, Offset(size.width * frac, size.height / 2))
        }
    }
}

/** A round control button: icon on TV/phone, cream when focused. */
@Composable
fun CtrlButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    big: Boolean = false,
) {
    FocusCard(onClick = onClick, shape = CircleShape, modifier = modifier.semantics { contentDescription = label }) { focused ->
        Box(
            Modifier
                .size(if (big) 64.dp else 48.dp)
                .background(if (focused) ReelColors.Ink else Color(0x33FFFFFF), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = if (focused) ReelColors.Bg else Color.White, modifier = Modifier.size(if (big) 36.dp else 26.dp))
        }
    }
}

/** One row of a menu sheet. */
data class MenuRow(val key: String, val title: String, val detail: String? = null, val selected: Boolean = false, val enabled: Boolean = true)

/**
 * A side sheet: audio, subtitles, quality, info. On TV the selected row gets focus. [onPick] gets
 * the row's key; [rows] empty plus [lines] makes an info sheet.
 */
@Composable
fun MenuSheet(
    title: String,
    rows: List<MenuRow>,
    onPick: (String) -> Unit,
    modifier: Modifier = Modifier,
    lines: List<String> = emptyList(),
) {
    val first = remember { FocusRequester() }
    Box(modifier.fillMaxHeight().width(380.dp).background(Color(0xF2131010)).padding(20.dp).testTag("menu-sheet")) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.headlineSmall, color = ReelColors.Ink)
            Spacer(Modifier.height(4.dp))
            for (l in lines) Text(l, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2)
            val focusIndex = rows.indexOfFirst { it.selected }.takeIf { it >= 0 } ?: 0
            LazyColumn(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                itemsIndexed(rows, key = { _, r -> r.key }) { i, r ->
                    FocusCard(
                        onClick = { if (r.enabled) onPick(r.key) },
                        shape = RoundedCornerShape(10.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .then(if (i == focusIndex) Modifier.focusRequester(first) else Modifier)
                            .testTag("menu-${r.key}"),
                    ) { focused ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .background(if (focused) ReelColors.Ink else Color.Transparent)
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            val ink = when {
                                focused -> ReelColors.Bg
                                !r.enabled -> ReelColors.Ink3
                                else -> ReelColors.Ink
                            }
                            Box(Modifier.size(22.dp)) {
                                if (r.selected) Icon(Icons.Filled.Check, contentDescription = "Selected", tint = if (focused) ReelColors.Bg else ReelColors.AccentHi)
                            }
                            Column(Modifier.weight(1f)) {
                                Text(r.title, style = MaterialTheme.typography.titleMedium, color = ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                r.detail?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = if (focused) ReelColors.Bg else ReelColors.Ink3) }
                            }
                        }
                    }
                }
            }
        }
    }
    LaunchedEffect(rows.size) { if (rows.isNotEmpty()) runCatching { first.requestFocus() } }
}

/** Tabular figures, so times don't jitter as they tick. */
val Tabular = androidx.compose.ui.text.TextStyle(fontFeatureSettings = "tnum")

/** "Direct play" in green, "Transcode · 720p" in yellow: what the server is doing. */
@Composable
fun ModeBadge(s: app.reel.android.data.DecideResponse, modifier: Modifier = Modifier) {
    val copy = s.mode == "direct_play" || s.mode == "remux"
    val text = when (s.mode) {
        "direct_play" -> "Direct play"
        "remux" -> "Direct stream"
        "audio_transcode" -> "Audio transcode"
        else -> listOfNotNull("Transcode", s.quality?.takeIf { it != "original" }).joinToString(" · ")
    }
    val c = if (copy) ReelColors.Ok else ReelColors.Warn
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        color = c,
        modifier = modifier.background(c.copy(alpha = 0.16f), RoundedCornerShape(50)).padding(horizontal = 12.dp, vertical = 5.dp),
    )
}

/** A pill: "Audio  EN 5.1". On TV the focused one is cream with dark text. */
@Composable
fun PillButton(
    label: String,
    value: String?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
) {
    val shape = RoundedCornerShape(50)
    FocusCard(onClick = onClick, shape = shape, modifier = modifier.semantics { contentDescription = listOfNotNull(label, value).joinToString(": ") }) { focused ->
        Row(
            Modifier
                .background(if (focused) ReelColors.Ink else ReelColors.Ink.copy(alpha = 0.1f), shape)
                .padding(horizontal = 16.dp, vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            val ink = if (focused) ReelColors.Bg else ReelColors.Ink
            icon?.let { Icon(it, contentDescription = null, tint = ink, modifier = Modifier.size(22.dp)) }
            Text(label, style = MaterialTheme.typography.labelLarge, color = ink)
            value?.let { Text(it, style = MaterialTheme.typography.labelLarge.merge(Tabular), color = if (focused) ReelColors.Bg.copy(alpha = 0.7f) else ReelColors.Ink2) }
        }
    }
}

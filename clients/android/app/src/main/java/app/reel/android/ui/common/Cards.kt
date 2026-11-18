package app.reel.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.foundation.progressSemantics
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.theme.ReelDimensions
import app.reel.android.ui.tv.FocusCard

/** Card widths per device kind (TV is read from across the room). */
object CardSize {
    @Composable
    fun poster(): Dp = if (LocalDeviceKind.current == DeviceKind.Tv) 150.dp else 112.dp

    @Composable
    fun landscape(): Dp = if (LocalDeviceKind.current == DeviceKind.Tv) 280.dp else 220.dp
}

/** 2:3 poster with the title under it. [progress] (0–1) draws a bar along the bottom. */
@Composable
fun PosterCard(
    title: String,
    imageUrl: String?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    progress: Double? = null,
    played: Boolean = false,
) {
    Column(modifier.width(CardSize.poster())) {
        FocusCard(onClick = onClick, modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f)) {
            ReelImage(imageUrl, contentDescription = title, modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f))
            if (played) WatchedBadge() else WatchProgress(progress, Modifier.align(Alignment.BottomStart).fillMaxWidth().padding(start = 4.dp, end = 4.dp, bottom = 4.dp))
        }
        CardCaption(title, subtitle)
    }
}

/** 16:9 still or backdrop with the title under it (Continue watching, episodes). */
@Composable
fun LandscapeCard(
    title: String,
    imageUrl: String?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    progress: Double? = null,
    played: Boolean = false,
) {
    Column(modifier.width(CardSize.landscape())) {
        FocusCard(onClick = onClick, modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f)) {
            ReelImage(imageUrl, contentDescription = title, modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f))
            if (played) WatchedBadge() else WatchProgress(progress, Modifier.align(Alignment.BottomStart).fillMaxWidth().padding(start = 4.dp, end = 4.dp, bottom = 4.dp))
        }
        CardCaption(title, subtitle)
    }
}

/** A check in the corner of a watched title. */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.WatchedBadge() {
    Box(
        Modifier.align(Alignment.TopEnd).padding(6.dp).size(22.dp).background(ReelColors.Accent, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(Icons.Outlined.Check, contentDescription = "Watched", tint = ReelColors.AccentInk, modifier = Modifier.size(16.dp))
    }
}

/** A compact, accessible playback progress track. Null, non-finite and zero values are hidden. */
@Composable
fun WatchProgress(progress: Double?, modifier: Modifier = Modifier) {
    val fraction = progress?.takeIf { it.isFinite() }?.coerceIn(0.0, 1.0)?.toFloat() ?: return
    if (fraction == 0f) return
    Box(modifier.height(ReelDimensions.ProgressHeight).progressSemantics(fraction)
        .background(ReelColors.Bg.copy(alpha = 0.8f))) {
        Box(Modifier.fillMaxWidth(fraction).height(ReelDimensions.ProgressHeight).background(ReelColors.AccentHi))
    }
}

@Composable
private fun CardCaption(title: String, subtitle: String?) {
    Text(
        title,
        style = MaterialTheme.typography.bodyMedium,
        color = ReelColors.Ink,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier.padding(top = 8.dp),
    )
    if (subtitle != null) {
        Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

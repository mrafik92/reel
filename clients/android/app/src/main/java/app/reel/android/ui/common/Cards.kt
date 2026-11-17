package app.reel.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.theme.ReelColors
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
) {
    Column(modifier.width(CardSize.poster())) {
        FocusCard(onClick = onClick, modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f)) {
            ReelImage(imageUrl, contentDescription = title, modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f))
            ProgressBar(progress)
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
) {
    Column(modifier.width(CardSize.landscape())) {
        FocusCard(onClick = onClick, modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f)) {
            ReelImage(imageUrl, contentDescription = title, modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f))
            ProgressBar(progress)
        }
        CardCaption(title, subtitle)
    }
}

@Composable
private fun androidx.compose.foundation.layout.BoxScope.ProgressBar(progress: Double?) {
    val p = progress?.toFloat()?.coerceIn(0f, 1f) ?: return
    if (p <= 0f) return
    Box(Modifier.align(Alignment.BottomStart).fillMaxWidth().height(4.dp).background(ReelColors.Bg.copy(alpha = 0.6f)))
    Box(Modifier.align(Alignment.BottomStart).fillMaxWidth(p).height(4.dp).background(ReelColors.Accent))
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

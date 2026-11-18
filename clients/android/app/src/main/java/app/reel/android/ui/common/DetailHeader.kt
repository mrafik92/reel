package app.reel.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.theme.ReelColors

@Composable
internal fun DetailHeader(
    title: String,
    kind: String,
    poster: String?,
    backdrop: String?,
    facts: List<String>,
    actions: @Composable () -> Unit,
) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    Box(Modifier.fillMaxWidth()) {
        val background = if (tv) backdrop ?: poster else backdrop
        if (background != null) {
            Box(Modifier.fillMaxWidth().height(if (tv) 340.dp else 196.dp)) {
                ReelImage(background, contentDescription = null, modifier = Modifier.fillMaxSize())
                if (tv || poster != null) {
                    Box(Modifier.matchParentSize().background(ReelColors.Bg.copy(alpha = 0.65f)))
                }
                Box(Modifier.matchParentSize().background(Brush.verticalGradient(listOf(Color.Transparent, ReelColors.Bg))))
            }
        }
        if (tv) {
            Row(
                Modifier.padding(start = 48.dp, end = 48.dp, top = 40.dp, bottom = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(32.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (poster != null) DetailPoster(poster, Modifier.width(172.dp))
                Box(Modifier.weight(1f)) { DetailHeading(title, kind, facts, actions) }
            }
        } else {
            Column(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                when {
                    poster != null -> DetailPoster(poster, Modifier.align(Alignment.CenterHorizontally).padding(top = 12.dp).width(120.dp))
                    backdrop != null -> Box(Modifier.height(150.dp))
                }
                Box(Modifier.padding(top = if (poster == null && backdrop == null) 24.dp else 0.dp)) {
                    DetailHeading(title, kind, facts, actions)
                }
            }
        }
    }
}

@Composable
private fun DetailPoster(url: String, modifier: Modifier) {
    ReelImage(url, contentDescription = null, modifier = modifier.aspectRatio(2f / 3f).clip(RoundedCornerShape(12.dp)))
}

@Composable
private fun DetailHeading(title: String, kind: String, facts: List<String>, actions: @Composable () -> Unit) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(kind, style = MaterialTheme.typography.labelLarge, color = ReelColors.AccentHi)
        Text(
            title,
            style = (if (tv) MaterialTheme.typography.displaySmall else MaterialTheme.typography.headlineMedium)
                .copy(fontWeight = FontWeight.ExtraBold),
            color = ReelColors.Ink,
            modifier = Modifier.testTag("item-title"),
        )
        if (facts.isNotEmpty()) {
            Text(facts.joinToString(" · "), style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink2)
        }
        Box(Modifier.padding(top = 6.dp)) { actions() }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun DetailActions(content: @Composable () -> Unit) {
    FlowRow(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

package app.reel.android.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import app.reel.android.BuildConfig
import app.reel.android.DeviceKind
import app.reel.android.R
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.theme.ReelTheme

/**
 * Placeholder root of the app (#185). Sign-in (#186) and the app shell (#187) replace it.
 */
@Composable
fun ReelRoot(kind: DeviceKind) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(ReelColors.Bg)
            .safeDrawingPadding(),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Image(
            painter = painterResource(R.drawable.logo),
            contentDescription = null,
            modifier = Modifier.size(if (kind == DeviceKind.Tv) 160.dp else 120.dp),
        )
        Text("Reel", style = MaterialTheme.typography.displaySmall, color = ReelColors.Ink)
        Text(
            text = (if (kind == DeviceKind.Tv) "Android TV" else "Phone") + " · v" + BuildConfig.VERSION_NAME,
            style = MaterialTheme.typography.bodyMedium,
            color = ReelColors.Ink3,
        )
    }
}

@Preview(widthDp = 960, heightDp = 540)
@Composable
private fun TvPreview() = ReelTheme(DeviceKind.Tv) { ReelRoot(DeviceKind.Tv) }

@Preview(widthDp = 360, heightDp = 780)
@Composable
private fun PhonePreview() = ReelTheme(DeviceKind.Phone) { ReelRoot(DeviceKind.Phone) }

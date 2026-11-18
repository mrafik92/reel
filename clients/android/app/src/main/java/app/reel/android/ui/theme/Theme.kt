package app.reel.android.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.reel.android.DeviceKind

/** Tokens from `packages/ui/src/styles/ui.css` (crimson + cream cinema palette, #177). */
object ReelColors {
    val Bg = Color(0xFF0B0909)
    val Bg1 = Color(0xFF131010)
    val Bg2 = Color(0xFF1B1716)
    val Bg3 = Color(0xFF29221F)
    val Ink = Color(0xFFF2E6D3)
    val Ink2 = Ink.copy(alpha = 0.76f)
    val Ink3 = Ink.copy(alpha = 0.54f)
    val Accent = Color(0xFFC2352D)
    val AccentHi = Color(0xFFF26352)
    val AccentInk = Color(0xFFFFF6EA)
    val AccentWash = Color(0xFF2A1513)

    /** Good news: ratings, *Direct play*. */
    val Ok = Color(0xFF7FD4A0)
    /** Caution: *Transcode*. */
    val Warn = Color(0xFFFFD23F)
}

/** Shared control geometry for artwork, the shell, and screen actions. */
object ReelDimensions {
    val TouchTarget = 48.dp
    val CardRadius = 8.dp
    val FocusOutline = 2.dp
    val ProgressHeight = 4.dp
}

private val scheme = darkColorScheme(
    primary = ReelColors.Accent,
    onPrimary = ReelColors.AccentInk,
    primaryContainer = ReelColors.AccentWash,
    onPrimaryContainer = ReelColors.Ink,
    secondary = ReelColors.AccentHi,
    background = ReelColors.Bg,
    onBackground = ReelColors.Ink,
    surface = ReelColors.Bg1,
    onSurface = ReelColors.Ink,
    surfaceVariant = ReelColors.Bg2,
    onSurfaceVariant = ReelColors.Ink2,
    outline = ReelColors.Ink3,
)

/** TV is read from ~3 m away: the same roles, a larger scale. */
private fun typography(kind: DeviceKind): Typography {
    val k = if (kind == DeviceKind.Tv) 1.25f else 1f
    return Typography(
        displaySmall = TextStyle(fontSize = (36 * k).sp, fontWeight = FontWeight.Bold),
        headlineSmall = TextStyle(fontSize = (24 * k).sp, fontWeight = FontWeight.SemiBold),
        titleMedium = TextStyle(fontSize = (16 * k).sp, fontWeight = FontWeight.SemiBold),
        bodyLarge = TextStyle(fontSize = (16 * k).sp),
        bodyMedium = TextStyle(fontSize = (14 * k).sp),
        labelLarge = TextStyle(fontSize = (14 * k).sp, fontWeight = FontWeight.Medium),
    )
}

@Composable
fun ReelTheme(kind: DeviceKind, content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = scheme, typography = typography(kind), content = content)
}

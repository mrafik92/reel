package app.reel.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.runtime.compositionLocalOf
import app.reel.android.data.ServerAddress
import app.reel.android.ui.theme.ReelColors
import coil.compose.AsyncImage

/**
 * An image from the server (posters, backdrops, logos). The server returns signed, resized
 * URLs, usually relative (`/api/v1/images/...?sig=`), so they're resolved against the server and
 * loaded **without** the token (Coil uses the plain OkHttp client, see `ReelApplication`).
 */
@Composable
fun ReelImage(
    url: String?,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    contentScale: ContentScale = ContentScale.Crop,
) {
    val server = LocalServerAddress.current
    AsyncImage(
        model = resolveImageUrl(server, url),
        contentDescription = contentDescription,
        contentScale = contentScale,
        modifier = modifier.background(ReelColors.Bg2),
    )
}

/** The signed-in server, for resolving relative image URLs. Provided by the app shell. */
val LocalServerAddress = compositionLocalOf<ServerAddress?> { null }

/** Absolute URLs pass through; `/path` is resolved against [server]. */
fun resolveImageUrl(server: ServerAddress?, url: String?): String? = when {
    url.isNullOrBlank() -> null
    url.startsWith("http://") || url.startsWith("https://") -> url
    server == null -> null
    else -> server.origin + (if (url.startsWith("/")) url else "/$url")
}

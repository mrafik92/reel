package app.reel.android.ui.vod

import android.net.Uri
import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavType
import androidx.navigation.compose.composable
import androidx.navigation.navArgument
import app.reel.android.data.Me
import app.reel.android.player.StartupTiming

/** IPTV VOD routes (#193). Registered by [vodGraph] for users with VOD access. */
object VodRoutes {
    const val BROWSE = "vod?type={type}"
    const val GRID = "vod/grid?type={type}&query={query}"
    const val MOVIE = "vod/movie/{id}"
    const val SERIES = "vod/series/{id}"
    const val PLAYER = "vod/play/{kind}/{id}?series={series}&start={start}&tap={tap}"

    /** [type] `movie` | `series`. */
    fun browse(type: String = "movie") = "vod?type=$type"

    /** [query] is a `/vod/movies` query string (a lane's *see all*), empty for everything. */
    fun grid(type: String, query: String = "") = "vod/grid?type=$type&query=${Uri.encode(query)}"

    fun movie(id: Long) = "vod/movie/$id"

    fun series(id: Long) = "vod/series/$id"

    /** [kind] `movie` | `episode`; episodes need their [seriesId]. */
    fun player(kind: String, id: Long, seriesId: Long? = null, startS: Double = 0.0) =
        "vod/play/$kind/$id?series=${seriesId ?: 0}&start=$startS" + StartupTiming.tap().let { if (it > 0) "&tap=$it" else "" }
}

fun NavGraphBuilder.vodGraph(user: Me) {
    if (!user.canWatchVod) return
    composable(VodRoutes.BROWSE, arguments = listOf(navArgument("type") { defaultValue = "movie" })) { e ->
        VodBrowseScreen(e.arguments?.getString("type") ?: "movie")
    }
    composable(
        VodRoutes.GRID,
        arguments = listOf(navArgument("type") { defaultValue = "movie" }, navArgument("query") { defaultValue = "" }),
    ) { e ->
        VodGridScreen(e.arguments?.getString("type") ?: "movie", e.arguments?.getString("query").orEmpty())
    }
    composable(VodRoutes.MOVIE, arguments = listOf(navArgument("id") { type = NavType.LongType })) { e ->
        VodMovieScreen(e.arguments?.getLong("id") ?: 0)
    }
    composable(VodRoutes.SERIES, arguments = listOf(navArgument("id") { type = NavType.LongType })) { e ->
        VodSeriesScreen(e.arguments?.getLong("id") ?: 0)
    }
    composable(
        VodRoutes.PLAYER,
        arguments = listOf(
            navArgument("kind") { defaultValue = "movie" },
            navArgument("id") { type = NavType.LongType },
            navArgument("series") {
                type = NavType.LongType
                defaultValue = 0L
            },
            navArgument("tap") { type = NavType.LongType; defaultValue = 0L },
            navArgument("start") {
                type = NavType.FloatType
                defaultValue = 0f
            },
        ),
    ) { e ->
        val a = e.arguments
        VodPlayerScreen(
            kind = a?.getString("kind") ?: "movie",
            id = a?.getLong("id") ?: 0,
            seriesId = a?.getLong("series")?.takeIf { it != 0L },
            startS = (a?.getFloat("start") ?: 0f).toDouble(),
            tapMs = a?.getLong("tap") ?: 0L,
        )
    }
}

/** Where to start: the saved position, or 0 when unwatched or (nearly) finished. */
fun vodResumeS(progress: Double?, runtimeS: Long?): Double {
    val p = progress ?: return 0.0
    val r = runtimeS ?: return 0.0
    return if (p > 0 && p < app.reel.android.player.RESUME_MAX_PROGRESS) p * r else 0.0
}

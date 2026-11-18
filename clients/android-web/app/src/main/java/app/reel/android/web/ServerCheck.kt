package app.reel.android.web

import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.IOException
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException

/** What the setup screen found at an address (`GET /api/v1/health`, no credentials). */
sealed interface CheckResult {
    data object Ok : CheckResult
    data object NotReel : CheckResult
    data object UnknownHost : CheckResult
    data object Tls : CheckResult
    data class Unreachable(val detail: String) : CheckResult
}

class ServerCheck(
    private val client: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(6, TimeUnit.SECONDS)
        .readTimeout(6, TimeUnit.SECONDS)
        .followRedirects(false)
        .build(),
) {
    /** Blocking; call off the main thread. */
    fun check(server: ServerAddress): CheckResult = try {
        client.newCall(Request.Builder().url(server.health).build()).execute().use { r ->
            val body = r.body?.string().orEmpty()
            if (r.isSuccessful && body.trimStart().startsWith("{")) CheckResult.Ok else CheckResult.NotReel
        }
    } catch (e: UnknownHostException) {
        CheckResult.UnknownHost
    } catch (e: SSLException) {
        CheckResult.Tls
    } catch (e: IOException) {
        CheckResult.Unreachable(e.javaClass.simpleName)
    }
}

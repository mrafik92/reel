package app.reel.android.data

import java.io.IOException
import java.io.InterruptedIOException
import java.net.SocketTimeoutException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.SerializationStrategy
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.serializer
import okhttp3.Call
import okhttp3.Callback
import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response

/**
 * The Reel `/api/v1` client (OkHttp + kotlinx.serialization).
 *
 * - The bearer token goes **only** to [server]'s origin, on every hop (also after a redirect). Media
 *   and image URLs are signed, so the player and Coil never need it.
 * - Failures are classified into [ReelError] like the web client (#90). A 401 on a signed-in call
 *   runs [onUnauthorized] (the app goes back to sign-in).
 * - Reads that got no answer are retried once after [retryDelayMs]; writes aren't (they may already
 *   have happened).
 */
class ReelApi(
    val server: ServerAddress,
    base: OkHttpClient,
    private val token: () -> String?,
    private val isOnline: () -> Boolean = { true },
    private val onUnauthorized: () -> Unit = {},
    private val readTimeoutMs: Long = 20_000,
    private val writeTimeoutMs: Long = 60_000,
    private val retryDelayMs: Long = 600,
) {
    /** [base] plus the bearer interceptor. A network interceptor, so it re-checks the origin on redirects. */
    val client: OkHttpClient = base.newBuilder().addNetworkInterceptor(BearerInterceptor(server, token)).build()

    // ---- endpoints used by sign-in (#186); screens add theirs through get/post/delete -----------

    suspend fun health(): Health = get("/health", auth = false)

    suspend fun setupStatus(): SetupStatus = get("/setup", auth = false)

    suspend fun login(body: LoginBody): LoginResponse = post("/auth/login", body, auth = false)

    suspend fun logout() = postEmpty("/auth/logout")

    suspend fun me(): Me = get("/me")

    // ---- generic calls ------------------------------------------------------------------------

    suspend inline fun <reified T> get(path: String, query: Map<String, Any?> = emptyMap(), auth: Boolean = true): T =
        request("GET", path, query, null, serializer<T>(), auth)

    suspend inline fun <reified B, reified T> post(path: String, body: B, auth: Boolean = true): T =
        request("POST", path, emptyMap(), encode(serializer<B>(), body), serializer<T>(), auth)

    /** `POST` without a body and without a response (204). */
    suspend fun postEmpty(path: String) {
        request("POST", path, emptyMap(), "", Unit.serializer(), true)
    }

    suspend fun delete(path: String) {
        request("DELETE", path, emptyMap(), null, Unit.serializer(), true)
    }

    fun <B> encode(s: SerializationStrategy<B>, body: B): String = ReelJson.encodeToString(s, body)

    /**
     * One API call. [path] is relative to `/api/v1`; `null` / empty [query] values are skipped.
     * [bodyJson] `null` sends no body. `Unit` responses accept any (or no) body.
     */
    suspend fun <T> request(
        method: String,
        path: String,
        query: Map<String, Any?>,
        bodyJson: String?,
        result: DeserializationStrategy<T>,
        auth: Boolean,
    ): T = withContext(Dispatchers.IO) {
        // Off the caller's thread: screens call from Main, and reading a body there throws
        // NetworkOnMainThreadException once it isn't already buffered.
        requestBlocking(method, path, query, bodyJson, result, auth)
    }

    private suspend fun <T> requestBlocking(
        method: String,
        path: String,
        query: Map<String, Any?>,
        bodyJson: String?,
        result: DeserializationStrategy<T>,
        auth: Boolean,
    ): T {
        val url = server.api(path).newBuilder().apply {
            for ((k, v) in query) if (v != null && v.toString().isNotEmpty()) addQueryParameter(k, v.toString())
        }.build()
        val req = Request.Builder()
            .url(url)
            .header("Accept", "application/json")
            .method(method, bodyJson?.toRequestBody(JSON))
            .apply { if (!auth) tag(NoAuth::class.java, NoAuth) }
            .build()

        val tries = if (method == "GET") 2 else 1
        var failure: ReelError? = null
        for (i in 0 until tries) {
            if (i > 0) delay(retryDelayMs)
            val res = try {
                execute(req, if (method == "GET") readTimeoutMs else writeTimeoutMs)
            } catch (e: CancellationException) {
                throw e
            } catch (e: IOException) {
                failure = classify(e)
                if (failure is ReelError.Offline) break
                continue
            }
            return res.use { handle(it, result, auth) }
        }
        throw failure ?: ReelError.Unreachable()
    }

    private fun classify(e: IOException): ReelError = when {
        !isOnline() -> ReelError.Offline(e)
        e is SocketTimeoutException -> ReelError.Timeout(e)
        // The call timeout surfaces as InterruptedIOException("timeout").
        e is InterruptedIOException && e.message == "timeout" -> ReelError.Timeout(e)
        else -> ReelError.Unreachable(e)
    }

    private fun <T> handle(res: Response, result: DeserializationStrategy<T>, auth: Boolean): T {
        val text = res.body?.string().orEmpty()
        if (!res.isSuccessful) {
            val msg = runCatching { ReelJson.decodeFromString(ErrorBody.serializer(), text).error }.getOrNull()
                ?.takeIf { it.isNotBlank() }
            when {
                res.code == 401 && auth -> {
                    onUnauthorized()
                    throw ReelError.Unauthorized()
                }
                res.code == 429 -> throw ReelError.RateLimited(
                    msg ?: ReelError.friendly(429),
                    res.header("Retry-After")?.trim()?.toLongOrNull()?.takeIf { it > 0 },
                )
                else -> throw ReelError.Http(res.code, msg ?: ReelError.friendly(res.code), text.ifEmpty { null })
            }
        }
        @Suppress("UNCHECKED_CAST")
        if (result == Unit.serializer()) return Unit as T
        return try {
            ReelJson.decodeFromString(result, text)
        } catch (e: SerializationException) {
            throw ReelError.BadResponse(e)
        } catch (e: IllegalArgumentException) {
            throw ReelError.BadResponse(e)
        }
    }

    private suspend fun execute(req: Request, timeoutMs: Long): Response {
        val call = client.newCall(req)
        call.timeout().timeout(timeoutMs, TimeUnit.MILLISECONDS)
        return suspendCancellableCoroutine { cont ->
            cont.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onFailure(call: Call, e: IOException) {
                    if (!cont.isCancelled) cont.resumeWithException(e)
                }

                override fun onResponse(call: Call, response: Response) {
                    cont.resume(response) { _, _, _ -> response.close() }
                }
            })
        }
    }

    /** Marks a request that must go out without the token (sign-in, health). */
    private object NoAuth

    /** Adds `Authorization: Bearer` only for the configured origin. */
    private class BearerInterceptor(private val server: ServerAddress, private val token: () -> String?) : Interceptor {
        override fun intercept(chain: Interceptor.Chain): Response {
            val req = chain.request()
            val t = token()
            val send = t != null && req.tag(NoAuth::class.java) == null && server.owns(req.url)
            val out = req.newBuilder().removeHeader("Authorization").apply {
                if (send) header("Authorization", "Bearer $t")
            }.build()
            return chain.proceed(out)
        }
    }

    private companion object {
        val JSON = "application/json".toMediaType()
    }
}

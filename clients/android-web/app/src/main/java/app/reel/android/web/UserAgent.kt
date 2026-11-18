package app.reel.android.web

/**
 * The shell's identity, appended to WebView's own user agent. The page can't call into the app (no
 * JavaScript bridge, #225), so this token is the only thing that tells the web client it runs inside
 * Reel Web for Android: `ReelAndroidWeb/<version>`, plus ` (TV)` on Android TV. #229 reads it.
 */
object UserAgent {
    const val PRODUCT = "ReelAndroidWeb"

    fun token(versionName: String, tv: Boolean): String = "$PRODUCT/$versionName" + if (tv) " (TV)" else ""

    fun extend(base: String, versionName: String, tv: Boolean): String {
        val stripped = base.replace(Regex("""\s*$PRODUCT/\S+( \(TV\))?"""), "").trimEnd()
        return "$stripped ${token(versionName, tv)}".trimStart()
    }
}

package app.reel.android.web

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/**
 * One APK for phones and Android TV (#231). Leanback and a touchscreen must stay optional, or the Play
 * Store and package installer refuse the APK on the other kind of device; the launcher activity needs
 * the Leanback category and the application a banner, or Android TV's home screen doesn't list it.
 */
class TvManifestTest {
    private val android = "http://schemas.android.com/apk/res/android"

    // Unit tests run with the module (`app/`) as the working directory.
    private val manifest: Element = DocumentBuilderFactory.newInstance()
        .apply { isNamespaceAware = true }
        .newDocumentBuilder()
        .parse(File("src/main/AndroidManifest.xml"))
        .documentElement

    private fun elements(tag: String): List<Element> {
        val nodes = manifest.getElementsByTagName(tag)
        return (0 until nodes.length).map { nodes.item(it) as Element }
    }

    private fun Element.attr(name: String): String = getAttributeNS(android, name)

    @Test
    fun leanbackAndTouchscreenAreOptional() {
        val features = elements("uses-feature").associate { it.attr("name") to it.attr("required") }
        assertEquals("false", features["android.software.leanback"])
        assertEquals("false", features["android.hardware.touchscreen"])
    }

    @Test
    fun launcherActivityIsOnBothHomeScreens() {
        val main = elements("activity").single { it.attr("name") == ".MainActivity" }
        val categories = main.getElementsByTagName("category").let { n ->
            (0 until n.length).map { (n.item(it) as Element).attr("name") }
        }
        assertTrue(categories.contains("android.intent.category.LAUNCHER"))
        assertTrue(categories.contains("android.intent.category.LEANBACK_LAUNCHER"))
    }

    @Test
    fun applicationHasTheTvBanner() {
        val app = elements("application").single()
        assertEquals("@drawable/tv_banner", app.attr("banner"))
        assertTrue(File("src/main/res/drawable-xhdpi/tv_banner.png").isFile)
    }
}

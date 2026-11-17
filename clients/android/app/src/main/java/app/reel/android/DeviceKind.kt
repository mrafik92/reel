package app.reel.android

import android.app.UiModeManager
import android.content.Context
import android.content.res.Configuration

/**
 * One APK runs on phones, tablets and Android TV. The device kind picks the layout (nav rail and
 * D-pad focus on TV, bottom bar and touch on phones), never the feature set.
 */
enum class DeviceKind {
    Tv,
    Phone;

    companion object {
        /** From `Configuration.uiMode` (or `UiModeManager.currentModeType`). */
        fun fromUiMode(uiMode: Int): DeviceKind =
            if (uiMode and Configuration.UI_MODE_TYPE_MASK == Configuration.UI_MODE_TYPE_TELEVISION) Tv else Phone

        fun of(context: Context): DeviceKind {
            val mgr = context.getSystemService(Context.UI_MODE_SERVICE) as UiModeManager
            return fromUiMode(mgr.currentModeType)
        }
    }
}

/** The device kind for the whole UI; provided once in [MainActivity]. */
val LocalDeviceKind = androidx.compose.runtime.staticCompositionLocalOf { DeviceKind.Phone }

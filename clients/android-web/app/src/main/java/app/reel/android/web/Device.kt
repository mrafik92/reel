package app.reel.android.web

import android.app.UiModeManager
import android.content.Context
import android.content.res.Configuration

/** True on Android TV (UI mode television). It only changes the user-agent token, not the shell. */
fun Context.isTelevision(): Boolean =
    (getSystemService(Context.UI_MODE_SERVICE) as? UiModeManager)?.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION

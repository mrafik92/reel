package app.reel.android.data

import kotlinx.serialization.json.Json

/**
 * One JSON configuration for the whole app. The server may add fields at any time, so unknown keys
 * are ignored; defaults are encoded (so `client: "android"` and the preset always go out) but
 * `null`s aren't (optional request fields are simply left out).
 */
val ReelJson: Json = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
    coerceInputValues = true
}

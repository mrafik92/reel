package app.reel.android.player

import android.view.KeyEvent

/** What a remote key does in the player (web parity: `Player.tsx` onKey / onRemoteKey). */
sealed interface KeyAction {
    data object TogglePlay : KeyAction

    data object Play : KeyAction

    data object Pause : KeyAction

    /** Seek by [seconds] (negative = back). [focusSeekBar]: wake the controls on the timeline. */
    data class Seek(val seconds: Int, val focusSeekBar: Boolean = false) : KeyAction

    /** Only show the controls (first press while they're hidden). */
    data object Wake : KeyAction

    data object Next : KeyAction

    data object Leave : KeyAction

    /** Not the player's: let focus handling have it (and keep the controls up). */
    data object Pass : KeyAction
}

object PlayerKeys {
    const val SEEK_S = 10
    const val FAST_FORWARD_S = 30

    /**
     * Map a key-down. Media keys always act; with the controls hidden the D-pad first wakes them
     * (left/right seek ±10 s straight away, like the web's TV mode); with the controls up the
     * D-pad moves focus. Back is handled by the screen (it peels menus and controls first).
     */
    fun map(keyCode: Int, controlsVisible: Boolean): KeyAction = when (keyCode) {
        KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_HEADSETHOOK -> KeyAction.TogglePlay
        KeyEvent.KEYCODE_MEDIA_PLAY -> KeyAction.Play
        KeyEvent.KEYCODE_MEDIA_PAUSE -> KeyAction.Pause
        KeyEvent.KEYCODE_SPACE -> if (controlsVisible) KeyAction.Pass else KeyAction.TogglePlay
        KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> KeyAction.Seek(FAST_FORWARD_S)
        KeyEvent.KEYCODE_MEDIA_REWIND -> KeyAction.Seek(-SEEK_S)
        KeyEvent.KEYCODE_MEDIA_SKIP_FORWARD, KeyEvent.KEYCODE_MEDIA_STEP_FORWARD -> KeyAction.Seek(FAST_FORWARD_S)
        KeyEvent.KEYCODE_MEDIA_SKIP_BACKWARD, KeyEvent.KEYCODE_MEDIA_STEP_BACKWARD -> KeyAction.Seek(-SEEK_S)
        KeyEvent.KEYCODE_MEDIA_NEXT -> KeyAction.Next
        KeyEvent.KEYCODE_MEDIA_STOP -> KeyAction.Leave
        KeyEvent.KEYCODE_DPAD_LEFT -> if (controlsVisible) KeyAction.Pass else KeyAction.Seek(-SEEK_S, focusSeekBar = true)
        KeyEvent.KEYCODE_DPAD_RIGHT -> if (controlsVisible) KeyAction.Pass else KeyAction.Seek(SEEK_S, focusSeekBar = true)
        KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN,
        KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER,
        -> if (controlsVisible) KeyAction.Pass else KeyAction.Wake
        else -> KeyAction.Pass
    }
}

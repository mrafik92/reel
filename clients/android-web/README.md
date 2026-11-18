# Reel Web for Android

A small Android app that shows your Reel server's own web app in a WebView (#225). It installs **next
to** the native Compose app (`clients/android`), so both can be compared on one phone:

| | Reel (Compose) | Reel Web |
|---|---|---|
| Package | `app.reel.android` | `app.reel.android.web` |
| Debug package | `app.reel.android.debug` | `app.reel.android.web.debug` |
| UI | native Compose screens | the server's web app, updated with each web deploy |
| Player | Media3 ExoPlayer | the web player in WebView |

Design and open work: [`docs/design/225-android-web.md`](../../docs/design/225-android-web.md).

## Build

Same toolchain as the Compose app (JDK 17–21 and the Android SDK; see `clients/android/README.md`):

```sh
cd clients/android-web
./gradlew lint testDebugUnitTest assembleDebug     # → app/build/outputs/apk/debug/app-debug.apk
REEL_URL=http://192.168.1.20:8420 ./gradlew assembleDebug   # optional: preset the server address
```

## Install and use

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

1. Open **Reel Web** (the dark icon with "WEB") and enter the address you use in a browser:
   `192.168.1.20:8420`, a Tailscale IP or MagicDNS name, or `https://reel.example.com`. Off your network?
   **Save anyway** keeps it.
2. Sign in on the Reel page. The session is kept until you sign out, also across restarts.
3. To switch servers, long-press the launcher icon → **Change server**, or use the button on the
   "Can't reach Reel" screen.

Links to other sites open in your browser. Servers with certificate errors are refused.

Back works like the web app's Back key: it leaves full-screen video first, then closes an open sheet or
menu, then goes to the previous page. On Home and on the sign-in page, Back leaves the app. Full-screen video
turns the phone to landscape and hides the system bars until you leave full screen.

### Android TV

The same APK installs on Android TV (checked on the emulator only, #231). It's listed under **Apps** with the
dark "Reel WEB" banner, and the web app switches to its 10-foot layout with D-pad navigation (needs a server
with #229 deployed). The remote's arrows, OK, Back and media keys go straight to the web app. There's no
long-press shortcut on TV: use **Change server** on the "Can't reach Reel" screen.

## Debugging

Debug builds allow remote debugging: with the phone or emulator on adb, open `chrome://inspect` in desktop
Chrome and pick the Reel Web page.

# Reel – Android app (TV + phone)

Native Kotlin app: Jetpack Compose UI and Media3/ExoPlayer. **One APK** runs on phones, tablets and Android TV.
It shows up in both the normal launcher and the Leanback (TV) launcher. Design and roadmap:
[`docs/design/29-android-app.md`](../../docs/design/29-android-app.md) (epic #29).

This is a standalone Gradle project. It isn't part of the bun workspaces in `clients/`.

## Toolchain (macOS)

```sh
brew install openjdk@21 gradle
brew install --cask android-commandlinetools
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$PATH"
yes | sdkmanager --licenses
sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0" "emulator" \
  "system-images;android-34;android-tv;arm64-v8a" "system-images;android-35;google_apis;arm64-v8a"
```

AGP 8.7 needs JDK 17–21. A newer default JDK (e.g. 27) fails, so set `JAVA_HOME` as above. CI uses JDK 17.

## Build and test

```sh
cd clients/android
./gradlew lint testDebugUnitTest assembleDebug     # what CI runs
# → app/build/outputs/apk/debug/app-debug.apk  (package app.reel.android.debug)
```

`REEL_URL=https://reel.example` at build time presets the server address (the same idea as `VITE_REEL_URL` for
the TV apps). `REEL_VERSION_NAME` / `REEL_VERSION_CODE` set the version.

CI (`ci.yml`, job `android`) runs the same command when `clients/android/**` changes and uploads the debug
APK as the `reel-android-debug` artifact.

## Emulators

```sh
avdmanager create avd -n reel-tv    -k "system-images;android-34;android-tv;arm64-v8a" -d tv_1080p
avdmanager create avd -n reel-phone -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
emulator -avd reel-tv &          # D-pad: arrow keys, Enter = OK, Esc = Back
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n app.reel.android.debug/app.reel.android.MainActivity
```

## Install on a real device

- **Phone:** enable *Developer options → USB debugging*, then `adb install -r app-debug.apk`.
- **Android TV:** *Settings → Device Preferences → About*, then press *Build* 7 times. Enable *Developer
  options → USB/Network debugging*. Then run `adb connect <tv-ip>:5555` and `adb install -r app-debug.apk`.

Signed release APKs on a tag come with #195.

## Icons

`app/src/main/res/mipmap-*`, `drawable-nodpi/logo.png` and `drawable-xhdpi/tv_banner.png` are generated from
`clients/packages/ui/assets/logo.svg`. Run `bun run icons` in `clients/` and don't edit them by hand.

## Layout

```
app/src/main/java/app/reel/android/
  ReelApplication.kt    makes the AppGraph once per process
  AppGraph.kt           process-wide objects (OkHttp, SessionManager); screens get it via LocalAppGraph
  MainActivity.kt       entry point; picks DeviceKind (TV or phone) once
  DeviceKind.kt         TV vs phone from the UI mode
  data/
    Models.kt           hand-written wire models (kotlinx.serialization); ModelsTest checks them
                        against server/openapi.json, so add every new model to its list
    Json.kt             ReelJson: ignores unknown fields, leaves nulls out of requests
    ServerAddress.kt    parses what the user typed into an origin (http for LAN-style, https otherwise)
    ReelApi.kt          /api/v1 client: bearer only to the server's origin, ReelError classes,
                        GET retried once; get/post/delete helpers for new endpoints
    ReelError.kt        Offline · Timeout · Unreachable · Unauthorized · RateLimited · Http · BadResponse
    SessionStore.kt     DataStore: server, token, cached user, per-install device id
    SessionManager.kt   AuthState (server → sign-in → signed in), connect / signIn / signOut, `api`
  ui/theme/Theme.kt     palette and type scale (tokens from packages/ui/src/styles/ui.css)
  ui/ReelRoot.kt        one screen per AuthState
  ui/auth/              server address and sign-in screens; a signed-in placeholder until the shell (#187)
```

Screens call the API through `LocalAppGraph.current.session.api` (`api.get<Home>("/home")`). A 401 there
signs out and shows the sign-in screen by itself. Show `ReelError.userMessage` for failures.

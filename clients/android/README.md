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
./gradlew lint testDebugUnitTest assembleDebug assembleRelease     # what CI runs
# → app/build/outputs/apk/debug/app-debug.apk  (package app.reel.android.debug)
```

`REEL_URL=https://reel.example` at build time presets the server address (the same idea as `VITE_REEL_URL` for
the TV apps). `REEL_VERSION_NAME` / `REEL_VERSION_CODE` set the version.

CI (`ci.yml`, job `android`) runs the same command plus an unsigned `assembleRelease` (so R8 problems show up
before a release tag) when `clients/android/**` changes, and uploads the debug APK as the `reel-android-debug`
artifact.

## Emulators

```sh
avdmanager create avd -n reel-tv    -k "system-images;android-34;android-tv;arm64-v8a" -d tv_1080p
avdmanager create avd -n reel-phone -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
emulator -avd reel-tv &          # D-pad: arrow keys, Enter = OK, Esc = Back
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n app.reel.android.debug/app.reel.android.MainActivity
```

## Install on a real device

Debug builds (from `assembleDebug` or the `reel-android-debug` CI artifact) install as `app.reel.android.debug`,
next to the release app.

- **Phone:** enable *Developer options → USB debugging*, then `adb install -r app-debug.apk`.
- **Android TV:** *Settings → Device Preferences → About*, then press *Build* 7 times. Enable *Developer
  options → USB/Network debugging*. Then run `adb connect <tv-ip>:5555` and `adb install -r app-debug.apk`.

## Release builds (#195)

Push a tag `android-vMAJOR.MINOR.PATCH` and `.github/workflows/android-release.yml` builds a signed, R8-shrunk
release APK, checks its signature and version, and attaches `reel-android-<version>.apk` plus `SHA256SUMS` to a
GitHub release of that tag.

```sh
git tag android-v0.2.0 && git push origin android-v0.2.0
```

**Version scheme** (`scripts/version-from-tag.sh`): `android-vMAJOR.MINOR.PATCH` gives versionName
`MAJOR.MINOR.PATCH` and versionCode `MAJOR*1_000_000 + MINOR*1_000 + PATCH`. For example `android-v0.2.0` →
0.2.0 / 2000 and `android-v1.10.3` → 1.10.3 / 1010003. MINOR and PATCH must be 0–999, with no leading zeros
or suffixes (`-rc1` is rejected), so every higher tag gets a higher versionCode and installs over the previous
APK. Local and debug builds default to versionCode 1.

**Signing.** Every release must be signed with the **same key**, or Android refuses to install it over the
previous one (you'd have to uninstall and lose the app's data). The workflow reads four Actions secrets and
fails with a clear error if any is missing:

| Secret | Contents |
|---|---|
| `ANDROID_KEYSTORE_B64` | the release keystore (`.jks`, PKCS12), base64 on one line |
| `ANDROID_KEYSTORE_PASSWORD` | the keystore password |
| `ANDROID_KEY_ALIAS` | the key alias (`reel`) |
| `ANDROID_KEY_PASSWORD` | the key password (same as the keystore password for PKCS12) |

The keystore is decoded into the runner's temp dir and deleted after the build. The passwords reach Gradle
only as environment variables (`REEL_KEYSTORE_FILE`, `REEL_KEYSTORE_PASSWORD`, `REEL_KEY_ALIAS`,
`REEL_KEY_PASSWORD`, read by `app/build.gradle.kts`), never as a file.

**One-time key setup (owner).** Run on the Mac, outside the repo. Keep the keystore and the password in your
password manager: if the key is lost, no future release can update installed apps.

```sh
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
mkdir -p ~/.reel-android && chmod 700 ~/.reel-android && cd ~/.reel-android
read -rs "PW?Keystore password: " && echo          # zsh; in bash: read -rsp 'Keystore password: ' PW
# PKCS12 uses one password for the store and the key. Passed via env, so it isn't in shell history or `ps`.
KS_PW="$PW" "$JAVA_HOME/bin/keytool" -genkeypair -v -keystore reel-release.jks -storetype PKCS12 \
  -alias reel -keyalg RSA -keysize 4096 -validity 10000 \
  -storepass:env KS_PW -keypass:env KS_PW -dname "CN=Reel, O=Reel"
chmod 600 reel-release.jks
# Save reel-release.jks and the password in the password manager now.

base64 -i reel-release.jks | tr -d '\n' | gh secret set ANDROID_KEYSTORE_B64 -R mrafik92/reel
printf '%s' "$PW" | gh secret set ANDROID_KEYSTORE_PASSWORD -R mrafik92/reel
printf '%s' "$PW" | gh secret set ANDROID_KEY_PASSWORD -R mrafik92/reel
printf '%s' reel  | gh secret set ANDROID_KEY_ALIAS -R mrafik92/reel
unset PW
gh secret list -R mrafik92/reel        # names only; the four ANDROID_* entries should be listed
```

**Local signed build.** Create `clients/android/keystore.properties` (git-ignored, like `*.jks`/`*.keystore`):

```properties
storeFile=/Users/<you>/.reel-android/reel-release.jks
storePassword=…
keyAlias=reel
keyPassword=…
```

Then `REEL_VERSION_NAME=0.2.0 REEL_VERSION_CODE=2000 ./gradlew assembleRelease` →
`app/build/outputs/apk/release/app-release.apk`. Without signing config the output is
`app-release-unsigned.apk`, which can't be installed. Check a build with
`$ANDROID_HOME/build-tools/35.0.0/apksigner verify --print-certs app-release.apk`.

## Install a release (sideload)

Download `reel-android-<version>.apk` from the GitHub release (the repo is private, so sign in, or use
`gh release download android-v0.2.0 -R mrafik92/reel -p '*.apk'`). A newer release installs over the older
one and keeps sign-in and settings.

**Phone or tablet**
- *Without a computer:* send the APK to the phone with Taildrop (Tailscale → *Send file*) or any file share,
  and open it from *Files*/*Downloads*. The first time, Android asks you to allow *Install unknown apps* for
  that app (Files, Chrome, Tailscale…). Allow it, go back and tap *Install*.
- *With adb:* enable USB debugging (see above), then `adb install -r reel-android-<version>.apk`.

**Android TV / Google TV**
- *With adb:* enable *Network debugging* (see above), then `adb connect <tv-ip>:5555`, accept the prompt on the
  TV, and run `adb install -r reel-android-<version>.apk`. Reel shows up in the TV's apps row.
- *With the Downloader app* (by AFTVnews, from the Play Store): the TV can't sign in to GitHub, so put the APK
  somewhere it can fetch on the LAN, e.g. run `python3 -m http.server 8000` in the APK's folder on the Mac and
  enter `http://<mac-ip>:8000/reel-android-<version>.apk` in Downloader. Allow *Install unknown apps* for
  Downloader when asked (*Settings → Apps → Security & restrictions*), install, then stop the server.
- *Taildrop* also works on Android TV if the Tailscale app is installed there.

## Icons

`app/src/main/res/mipmap-*`, `drawable-nodpi/logo.png` and `drawable-xhdpi/tv_banner.png` are generated from
`clients/packages/ui/assets/logo.svg`. Run `bun run icons` in `clients/` and don't edit them by hand.

## Layout

```
app/src/main/java/app/reel/android/
  MainActivity.kt       entry point; picks DeviceKind (TV or phone) once
  DeviceKind.kt         TV vs phone from the UI mode
  ui/theme/Theme.kt     palette and type scale (tokens from packages/ui/src/styles/ui.css)
  ui/ReelRoot.kt        placeholder root until sign-in (#186) and the app shell (#187)
```

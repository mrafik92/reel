# Reel Web for Android

Reel's Android app for phones, tablets and Android TV (#225): a small native shell that shows your Reel
server's own web app in a WebView, so the UI and the player are the web's and update with each web deploy.
**One APK** runs on phones and TVs and shows up in both the normal and the Leanback (TV) launcher.

| | |
|---|---|
| Package | `app.reel.android.web` (never change it: installed releases only update under this id) |
| Debug package | `app.reel.android.web.debug` |
| UI | the server's web app |
| Player | the web player in WebView |

It replaced the native Compose app (epic #29), which the owner dropped on 2026-11-19 and which was removed in
#260; that app is in git history only. Design and open work:
[`docs/design/225-android-web.md`](../../docs/design/225-android-web.md).

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

## Build

```sh
cd clients/android-web
./gradlew lint testDebugUnitTest assembleDebug     # → app/build/outputs/apk/debug/app-debug.apk
REEL_URL=http://192.168.1.20:8420 ./gradlew assembleDebug   # optional: preset the server address
```

`REEL_VERSION_NAME` / `REEL_VERSION_CODE` set the version (local and debug builds default to versionCode 1).
CI (`ci.yml`, job `android-web`) runs `lint testDebugUnitTest assembleDebug assembleRelease` when
`clients/android-web/**` changes. GitHub Actions is unavailable for now, so run it locally; PRs get no CI.

### Emulators

```sh
avdmanager create avd -n reel-tv    -k "system-images;android-34;android-tv;arm64-v8a" -d tv_1080p
avdmanager create avd -n reel-phone -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
emulator -avd reel-phone &       # on reel-tv the D-pad is the arrow keys, Enter = OK, Esc = Back
```

## Install and use

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Debug builds install as `app.reel.android.web.debug`, next to a release build. On a real phone, enable
*Developer options → USB debugging* first. On Android TV: *Settings → Device Preferences → About*, press *Build*
7 times, enable *Developer options → USB/Network debugging*, then `adb connect <tv-ip>:5555`.

1. Open **Reel Web** (the dark icon with "WEB") and enter the address you use in a browser:
   `192.168.1.20:8420`, a Tailscale IP or MagicDNS name, or `https://reel.example.com`. Off your network?
   **Save anyway** keeps it.
2. Sign in on the Reel page. The session is kept until you sign out, also across restarts.
3. To switch servers, long-press the launcher icon → **Change server**, or use the button on the
   "Can't reach Reel" screen.

Links to other sites open in your browser. Servers with certificate errors are refused.

Back works like the web app's Back key: it leaves full-screen video first, then closes an open sheet or
menu, then goes to the previous page. On Home and on the sign-in page, Back leaves the app. Full-screen video
turns the phone to landscape and hides the system bars until you leave full screen. Media keys and a wired
headset's button play and pause the video while the app is open.

### Android TV

The same APK installs on Android TV (checked on the emulator only, #231). It's listed under **Apps** with the
dark "Reel WEB" banner, and the web app switches to its 10-foot layout with D-pad navigation (needs a server
with #229 deployed). The remote's arrows, OK, Back and media keys go straight to the web app. There's no
long-press shortcut on TV: use **Change server** on the "Can't reach Reel" screen.

## Release builds (#232)

GitHub Actions is unavailable for now, so release APKs are built on the Mac and sent to the phone with
Taildrop. From the repo root:

```sh
scripts/android-release.sh web --taildrop                    # next patch after the newest android-web-v* tag
scripts/android-release.sh web --version 0.2.0 --taildrop    # or a chosen version
scripts/android-release.sh web --taildrop --github-release   # also tag and publish a GitHub release
```

It runs lint and the unit tests, builds a signed, R8-shrunk APK, checks the signature and version, and writes
`dist/ReelWeb-X.Y.Z.apk` plus `dist/ReelWeb-X.Y.Z.apk.sha256` (git-ignored). `--dry-run` builds but only prints
the Taildrop and GitHub commands. `scripts/android-release.sh --help` lists every option. The script's unit
tests: `scripts/tests/android-release-test.sh`.

**Version scheme** (`scripts/version-from-tag.sh`): `android-web-vMAJOR.MINOR.PATCH` gives versionName
`MAJOR.MINOR.PATCH` and versionCode `MAJOR*1_000_000 + MINOR*1_000 + PATCH`. For example `android-web-v0.2.0` →
0.2.0 / 2000 and `android-web-v1.10.3` → 1.10.3 / 1010003. MINOR and PATCH must be 0–999, with no leading zeros
or suffixes (`-rc1` is rejected), so every higher tag gets a higher versionCode and installs over the previous
APK. Without `--version` the script takes the next patch after the newest `android-web-v*` tag; the Compose app's
old `android-v*` tags are ignored.

**Signing.** Every release must be signed with the **same key**, or Android refuses to install it over the
previous one (you'd have to uninstall and lose the app's data). The key is `~/.reel-android/reel-release.jks`
(PKCS12, alias `reel`, certificate SHA-256 `a48654758e7b9b85a5825e7f7a55bfebc8b2d5370691d7d056124ee4c9a58ecb`;
it also signed the Compose app). The keystore and its password are backed up in the owner's Proton Pass (vault
*Personal*, item *Reel Android release key*; the keystore is a base64 hidden field, restore with `base64 -d`).
The script reads the password from `REEL_KEYSTORE_PASSWORD` (and `REEL_KEY_PASSWORD`, which defaults to it), or
from the macOS Keychain, or asks with a hidden prompt, and passes it to `keytool` and Gradle only as environment
variables (`REEL_KEYSTORE_FILE`, `REEL_KEYSTORE_PASSWORD`, `REEL_KEY_ALIAS`, `REEL_KEY_PASSWORD`, read by
`app/build.gradle.kts`), never as a file or an argument. To keep it in the Keychain (once; `-w` last makes
`security` prompt for it, so it isn't in your shell history):

```sh
security add-generic-password -s reel-android-keystore -a reel -w
```

Without a password the script stops before building. A key other than the release key gets a warning (its APK
can't update an installed release), and `--github-release` refuses it.

**Plain Gradle.** A git-ignored `clients/android-web/keystore.properties` (`storeFile`, `storePassword`,
`keyAlias`, `keyPassword`) also signs `./gradlew assembleRelease`, but it puts the password on disk; prefer the
script. Without any signing config the output is `app-release-unsigned.apk`, which can't be installed.

**Creating the key** (done once, in 2026-11; only needed again if it's lost, and then every installed app has to
be uninstalled). Run outside the repo:

```sh
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
mkdir -p ~/.reel-android && chmod 700 ~/.reel-android && cd ~/.reel-android
read -rs "PW?Keystore password: " && echo          # zsh; in bash: read -rsp 'Keystore password: ' PW
# PKCS12 uses one password for the store and the key. Passed via env, so it isn't in shell history or `ps`.
KS_PW="$PW" "$JAVA_HOME/bin/keytool" -genkeypair -v -keystore reel-release.jks -storetype PKCS12 \
  -alias reel -keyalg RSA -keysize 4096 -validity 10000 \
  -storepass:env KS_PW -keypass:env KS_PW -dname "CN=Reel, O=Reel"
chmod 600 reel-release.jks
unset PW
# Save reel-release.jks and the password in the password manager now.
```

**Taildrop.** `--taildrop` sends the APK to `rafiiiki` (`--taildrop-target NAME` for another device). If the
phone is offline, the script says so, prints the command to send it later, and exits 3. On the phone, tap the
Tailscale notification (or open it from *Files → Downloads*) and install. The first time, Android asks you to
allow *Install unknown apps* for that app.

**GitHub releases and Obtainium.** `--github-release` tags the current commit `android-web-vX.Y.Z`, pushes the
tag and creates the release "Reel Web X.Y.Z" with the APK and its `.sha256` (`gh release create`; it needs a
clean tree on `origin/main`). The discarded Compose app's old releases (`android-v0.1.0`–`0.3.0`, "Android
X.Y.Z") are still on the repo, so in Obtainium add the repo `mrafik92/reel` with a GitHub token (it's private)
and set:

- *Filter release titles by regular expression*: `^Reel Web `
- *Filter APKs by regular expression*: `^ReelWeb-.*\.apk$`

## Install a release (sideload)

The usual way is `scripts/android-release.sh web --taildrop` (above), which builds the APK and sends it to the
phone. Published releases are on GitHub: download `ReelWeb-<version>.apk` from the release (the repo is private,
so sign in, or use `gh release download android-web-v<version> -R mrafik92/reel -p '*.apk'`). A newer release
installs over the older one and keeps sign-in and settings.

**Phone or tablet**
- *Without a computer:* send the APK to the phone with Taildrop (Tailscale → *Send file*) or any file share,
  and open it from *Files*/*Downloads*. The first time, Android asks you to allow *Install unknown apps* for
  that app (Files, Chrome, Tailscale…). Allow it, go back and tap *Install*.
- *With adb:* enable USB debugging (see above), then `adb install -r ReelWeb-<version>.apk`.

**Android TV / Google TV**
- *With adb:* enable *Network debugging* (see above), then `adb connect <tv-ip>:5555`, accept the prompt on the
  TV, and run `adb install -r ReelWeb-<version>.apk`. Reel Web shows up in the TV's apps row.
- *With the Downloader app* (by AFTVnews, from the Play Store): the TV can't sign in to GitHub, so put the APK
  somewhere it can fetch on the LAN, e.g. run `python3 -m http.server 8000` in the APK's folder on the Mac and
  enter `http://<mac-ip>:8000/ReelWeb-<version>.apk` in Downloader. Allow *Install unknown apps* for
  Downloader when asked (*Settings → Apps → Security & restrictions*), install, then stop the server.
- *Taildrop* also works on Android TV if the Tailscale app is installed there.

## Icons

`app/src/main/res/mipmap-*`, `drawable-nodpi/logo.png` and `drawable-xhdpi/tv_banner.png` are generated from
`clients/packages/ui/assets/logo.svg` (a "WEB" variant on the dark background). Run `bun run icons` in
`clients/` and don't edit them by hand.

## Debugging

Debug builds allow remote debugging: with the phone or emulator on adb, open `chrome://inspect` in desktop
Chrome and pick the Reel Web page.

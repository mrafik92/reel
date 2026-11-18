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

## Release builds (#232)

GitHub Actions is unavailable for now, so release APKs are built on the Mac and sent to the phone with
Taildrop. From the repo root:

```sh
scripts/android-release.sh web --taildrop                    # next patch after the newest android-web-v* tag
scripts/android-release.sh web --version 0.2.0 --taildrop    # or a chosen version
scripts/android-release.sh web --taildrop --github-release   # also tag and publish a GitHub release
```

It runs lint and the unit tests, builds a signed, R8-shrunk APK, checks the signature and version, and writes
`dist/ReelWeb-X.Y.Z.apk` plus `dist/ReelWeb-X.Y.Z.apk.sha256` (git-ignored). The version code follows the
Compose app's scheme (`X*1_000_000 + Y*1_000 + Z`). `--dry-run` builds but only prints the Taildrop and GitHub
commands. `scripts/android-release.sh --help` lists every option.

**Signing.** Reel Web uses the Compose app's release key (`~/.reel-android/reel-release.jks`, alias `reel`), so
each release installs over the previous one. The password is in the owner's password manager. The script reads
it from `REEL_KEYSTORE_PASSWORD`, or from the macOS Keychain, or asks with a hidden prompt; it's never written
to a file. To keep it in the Keychain (once; `-w` last makes `security` prompt for it, so it isn't in your shell
history):

```sh
security add-generic-password -s reel-android-keystore -a reel -w
```

Without a password the script stops before building. A key other than the release key gets a warning (its APK
can't update an installed release), and `--github-release` refuses it.

**Taildrop.** `--taildrop` sends the APK to `rafiiiki` (`--taildrop-target NAME` for another device). If the
phone is offline, the script says so, prints the command to send it later, and exits 3. On the phone, tap the
Tailscale notification (or open it from *Files → Downloads*) and install. The first time, Android asks you to
allow *Install unknown apps* for that app.

**GitHub releases and Obtainium.** `--github-release` tags the current commit `android-web-vX.Y.Z`, pushes the
tag and creates the release "Reel Web X.Y.Z" with the APK and its `.sha256` (`gh release create`; it needs a
clean tree on `origin/main`). The Compose app's releases (`android-vX.Y.Z`, "Android X.Y.Z") share the repo, so
in Obtainium add the repo `mrafik92/reel` with a GitHub token (it's private) and set:

- *Filter release titles by regular expression*: `^Reel Web `
- *Filter APKs by regular expression*: `^ReelWeb-.*\.apk$`

## Debugging

Debug builds allow remote debugging: with the phone or emulator on adb, open `chrome://inspect` in desktop
Chrome and pick the Reel Web page.

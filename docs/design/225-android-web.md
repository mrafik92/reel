# Design: Reel Web for Android, a WebView client (#225)

> **Status (2026-11-18).** The shell (#226) is built: server setup, an origin-scoped WebView, the TLS and
> unreachable-server screens, and CI. Back, insets and full screen (#227), the web-side identity (#229) and
> recovery (#228: load timeout, plain error messages, automatic retry) are built. Release builds (#232) are built
> locally with `scripts/android-release.sh`, but none is published yet. The playback check (#230) and the TV
> check (#231) are open. When a part
> ships, FEATURES.md and ARCHITECTURE.md describe it and this note keeps what's still open.

Epic #225 · milestone **M5 – Mobile** · sub-issues #226–#232.

## 1. The decision in one paragraph

Reel gets a second Android client, **Reel Web** (`clients/android-web`, package `app.reel.android.web`). It's
a small native shell around an Android `WebView` that loads the Reel web app **from the configured server**.
It installs next to the Compose app (#29, `app.reel.android`), so the owner can compare both on one phone.
The goal is one Reel UI across web, webOS, Tizen and Android: browsing, global search, IPTV movies and series,
Discover, requests and download status come from the SolidJS client, and a web deploy updates them on Android
without an APK release. Playback uses the web player inside WebView. A Media3 handoff is decided later, from
the failures the playback check (#230, §8) records.

## 2. Shape of the code

```
clients/android-web/                 standalone Gradle project (no Compose, no bun workspace)
└── app/src/main/java/app/reel/android/web/
    ├── SetupActivity      server address entry, `GET /api/v1/health` check, "Save anyway"
    ├── MainActivity       the WebView, Back, error panel, load timeout, renderer-crash recovery, launcher shortcut
    ├── Recovery           LoadErrors (error → plain message, the 20 s timeout) and RetryPolicy (backoff)
    ├── NetworkWatch       default-network callback, started and stopped with the activity
    ├── ServerAddress      input → origin (same rules as the Compose app's ServerAddress)
    ├── ServerStore        the origin in app-private SharedPreferences (no backup)
    ├── ServerCheck        the setup screen's health check (OkHttp)
    ├── LinkPolicy         in-app / external / blocked for every navigation
    └── UserAgent          the `ReelAndroidWeb/<version>` token
```

| Concern | Choice | Why |
|---|---|---|
| UI | Android Views (two layouts) | Two native screens don't justify Compose; the APK stays ~3 MB |
| Web content | The server's own web app at `/` | No bundled copy of the UI to keep in sync (unlike webOS/Tizen, which package `apps/web`) |
| Storage | SharedPreferences (server), WebView storage (session) | The web app keeps its token in localStorage, so sign-in persists like a browser tab |
| Package id | `app.reel.android.web` (`.debug` suffix for debug) | Installs next to the Compose app's debug and release builds |
| Min SDK | 24 | Same as the Compose app; WebView updates through the Play Store |

## 3. Server and session

- The setup screen parses the address like the Compose app: LAN-style input (an IP, a bare host, `.local`,
  `.lan` or any explicit port) gets `http://`, dotted names get `https://`. It checks `/api/v1/health` and
  expects JSON. If the phone is off the LAN or Tailscale the check fails; **Save anyway** keeps the address.
  A TLS failure can't be saved past.
- `REEL_URL` at build time presets the address, as in the Compose app.
- The session is the web app's own: `reel.token` and `reel.device_id` in the origin's localStorage. It
  survives force stops and restarts. Each origin has its own storage, so a LAN address and a Tailscale address
  are separate sign-ins, and switching back to one keeps its session (checked in #228, §6).
- **Change server**: long-press the launcher icon → *Change server* (a dynamic shortcut), or the button on
  the error screen. The page can't open native UI (no bridge).
- Cleartext HTTP is allowed for any host, as in the Compose app: the user types the address, and private
  Reel servers are usually plain `http://…:8420`.

## 4. Security model

- **Origin scope** (`LinkPolicy`): top-level navigation stays in the WebView only for the configured scheme,
  host and port. Other `http(s)` links, `mailto:` and `tel:` open in the system (`ACTION_VIEW`,
  `CATEGORY_BROWSABLE`). Everything else (`intent:`, `javascript:`, `file:`, `content:`, `data:`, app
  schemes) is dropped. Embedded frames may load `http(s)` (trailers), nothing else. `target=_blank` and
  `window.open` navigate the same WebView (multiple windows off), so they pass through the same policy.
- **TLS**: `onReceivedSslError` always cancels. There's no "proceed anyway".
- **No JavaScript bridge** (`addJavascriptInterface` is never called). The page learns it runs in the shell
  only from the user-agent token (§7).
- File and content access off, mixed content never allowed, geolocation off, third-party cookies off, no
  backups (`allowBackup=false`, data-extraction rules exclude everything). WebView debugging only in debug builds.
- A renderer crash replaces the WebView and shows a reload screen instead of killing the app.

## 5. Android integration (#227)

Built (🧪, Android 15 phone emulator, WebView 124, mock web app from a dev build and the live server).

**Back** goes through the page first, because only the page knows about its open sheets and its player
layers. Order (`BackPolicy`):

1. Full-screen video → leave full screen. The player stays and keeps playing.
2. The native error panel is up → `WebView.goBack()`, or leave on the first page.
3. Otherwise the shell sends the page an **Escape** key (`WebView.dispatchKeyEvent`). Escape is already the
   web app's Back key on every platform (`normalizeKey`): the newest `onBack` handler wins (sheets, pickers,
   the player's seek preview → info → controls → exit), and the Shell's default goes up with
   `history.back()` on any route except `/`.
4. If the page leaves Escape unhandled, WebView returns it through `onUnhandledKeyEvent` (the shell tags its
   keys by `downTime`, `PageKeys`). Then the shell goes back in the WebView history when it can and the page
   isn't `/`, and otherwise leaves (`moveTaskToBack`; the page and session stay in memory).

When a text field has focus, the keyboard is already closed (the IME takes the press that closes it) and
the web app's Escape in a field only blurs it. So the shell sends one extra Escape first, and the press still
goes back, as in Chrome. The web app returns "unhandled" on `/`, and on sign-in/setup while signed out, so
Back leaves the app there (before, Back on the sign-in page redirected back to itself).

There's no JavaScript bridge. The only signal from the page is whether it called `preventDefault()` on the key.
Predictive back (`enableOnBackInvokedCallback`) is on, but the callback is always enabled because the page
decides after the press. So Android shows no back-to-home preview.

**History without a gesture.** Chromium marks history entries made without a user gesture as skippable, and
`WebView.canGoBack()`/`goBack()` skip them. The page's own `history.back()` doesn't. Checked: two
scripted (CDP `click()`) navigations, then two real Back presses, walk back through both. So routing Back
through the page fixes this for every Reel route. Web flows that push without a gesture are rare anyway: auth
redirects and next-episode autoplay use `replace`, and the player's exit uses `history.back()`.

**Full screen** (`FullScreen`): `onShowCustomView` puts WebView's view into a black layer over the page.
That layer isn't padded for insets, so the picture fills the screen. The system bars are hidden (a swipe
shows them briefly) and a phone turns to landscape (`SENSOR_LANDSCAPE`, even with auto-rotate off; a TV keeps
its orientation). Bars and the previous orientation come back on each exit:

- Back
- the player's full-screen button
- the player's exit arrow (its cleanup leaves full screen)
- a renderer crash

When a title ends, the web player keeps full screen and shows its end controls. That's the web app's
design, and the shell follows the page.

**Search → title → Back** shows the same query and results, the same scroll position, and focus on the card
that was opened, without the keyboard:

- `usePageFocus` saves under the address the page has when it's left. Search rewrites `?q=` with
  `replace`, so before this fix it saved under `/search` and restored nothing.
- Search restores once the query's results are back.
- With no saved card, Search doesn't focus its box on phone or desktop when it opens on a query.
- On TV, focus returns to the card and arrows continue from there.

Checked with Playwright: desktop 1440×900, 1920×1080 TV mode with arrow keys and Escape, and 412×915.
Also checked in the WebView with real taps.

**Keyboard and insets**: the page is padded by the system bars, the cutout and the IME (#226). Checked:
- Sign-in and Search inputs stay above the keyboard. The password field scrolls into view.
- After the keyboard hides (Back, the IME's hide key, rotating with it open, or tapping a result with it
  open), the page gets its full height back.

The "bottom half stays black after hiding the keyboard" report didn't reproduce in any of these paths.

**Rotation**: `configChanges` keeps the activity, so rotating during playback keeps the page and the
position. Checked: playback kept advancing across three rotations.

Still open: the setup screen's keyboard (native, unchanged) on a small phone, and a real phone.

## 6. Recovery (#228)

Built (🧪, ARCHITECTURE A66; Android 15 phone emulator, WebView 124, the live server and a dev build of the web app).

**Load timeout.** Chromium waits for the OS connect timeout on an address that never answers (about two
minutes on Android), with a blank page and a slow progress bar. The shell times every main-frame document
load it starts (`loadUrl`, Retry, Back from the panel, in-app link navigations) until the server answers:
the navigation commits (`onPageStarted`, `doUpdateVisitedHistory`) or fails (`onReceivedError`, HTTP 5xx,
TLS). After **20 s** without an answer it stops the load and shows *Can't reach Reel* (`LoadErrors.LOAD_TIMEOUT_MS`).

Why 20 s: the document is the web app's small `index.html`, so a slow but working path answers in a few
seconds. That includes a cold Tailscale path relayed through a distant DERP server, a WireGuard handshake
after sleep, and a phone radio waking up. 20 s leaves several times that margin and is still about a sixth
of Chromium's wait. Only the server's first answer is timed. The scripts, images and API calls that follow
are the web app's and aren't timed. On the emulator, `10.255.255.1:8420` shows the panel after 20 s, and the
live server commits about 0.5–0.9 s after `loadUrl`.

A late `onPageFinished` from a stopped load must not cancel the next load's timeout, so only a commit or an
error stops the timer. A page's own `location.reload()` (the admin *About* page's Reload button) isn't timed:
WebView gives the shell no hook for it.

**Plain-language errors** (`LoadErrors.classify`, unit-tested). Main-frame errors map to one message each. The
raw `net::ERR_*` name (or `HTTP 502`, or *No answer in 20 s*) is a small detail line under it:

| Kind | From | Title |
|---|---|---|
| Offline | no default network, `ERR_INTERNET_DISCONNECTED` | *You're offline* |
| Unknown host | `ERR_NAME_NOT_RESOLVED`, `ERROR_HOST_LOOKUP` | *Can't find the server* (suggests home Wi-Fi or Tailscale) |
| Refused | `ERR_CONNECTION_REFUSED` | *Reel isn't answering* (check the server and port) |
| Timed out | the shell's timeout, `ERR_CONNECTION_TIMED_OUT`, `ERR_ADDRESS_UNREACHABLE` | *Can't reach Reel* |
| Blocked port | `ERR_UNSAFE_PORT` (Chromium never opens ports such as 1–1023, 6000, 10080) | *Port not allowed* (use another port) |
| TLS | `onReceivedSslError`, `ERR_CERT_*`, `ERR_SSL_*` | *Certificate not trusted* |
| Server error | HTTP 5xx on the document | *Reel had a problem* |
| Other | reset, closed, empty response | *Can't reach Reel* |
| Crash | renderer gone | *The page stopped* |

A 4xx document is the web app's (its router and sign-in handle it). Subresource errors (one image or API
call) never take over the screen.

**Automatic retry** (`RetryPolicy`, `NetworkWatch`). `NetworkWatch` registers a default-network callback in
`onStart` and unregisters it in `onStop`. It only acts while the error panel is up:

- A network that becomes the default, or gains internet validation, starts a new run: retries after
  1, 2, 4, 8, 16 and 32 s (about a minute plus load times). Then the panel waits for the next network change
  or a tap. This covers airplane mode off, joining Wi-Fi, and Tailscale connecting, which is a new VPN
  default network. A LAN without internet never validates, but its "available" event still counts.
- While offline, the panel says *Waiting for a network connection* and doesn't poll.
- A failed attempt continues the backoff. A tap on Retry starts a new run. A network event during a retry
  that's still loading is ignored, because Wi-Fi reports "available" and then "validated".
- TLS errors, blocked ports and renderer crashes aren't retried automatically.
- The panel stays up while a retry loads, with *Connecting…* or *Trying again in 4 s*, so Chromium's own
  error page never shows. Focus goes to Retry only when the panel first appears, so a failed automatic
  retry doesn't move D-pad focus.
- A working page is never reloaded because the network changed. Checked: airplane mode on and off and
  Wi-Fi off and on, with the live server's page open, keep the page and its JavaScript state.

Checked on the emulator:
- Starting in airplane mode shows *You're offline*. Turning airplane mode off loads the page about 1 s
  later without a tap (recording linked from the PR).
- `10.255.255.1:8420` shows *Can't reach Reel* after 20 s.
- `10.0.2.2:8421` shows *Reel isn't answering*.
- `nosuchhost.invalid:8420` shows *Can't find the server*.
- `10.0.2.2:9` shows *Port not allowed*.
- *Change server* from the panel works offline (*Save anyway*), and the new server loads when the network returns.

**No stale page.** A server change starts a new task (`FLAG_ACTIVITY_CLEAR_TASK`), so the new server gets a
fresh WebView with an empty history. Checked: `history.length` is 1 after switching. As a backstop,
`onStart` replaces the WebView if the stored server differs from the page's, and a page restored from saved
state is replaced when it isn't on the configured origin. Sign-out is the web app's: it lands on `/login`
with the token gone, and Back doesn't return to the signed-in page. The first Back press went to the
auto-focused username field and the second left the app; that's the #227 area.

**LAN ↔ Tailscale sessions** (§3). Checked: signed in on a LAN-style origin (`http://<mac>:5181`, the web
dev server) and on `https://reel.rafikii.co`, switched between them twice, and each origin was still
signed in when it came back. A new origin starts signed out.

**Cold start** (process start → first page finished, `Process.getStartElapsedRealtime` to `onPageFinished`,
logged in debug builds; Android 15 emulator on an M-series Mac, live server over the emulator's NAT, signed in):

| Case | Runs (ms) |
|---|---|
| Force stop, warm HTTP cache | 1324, 1323, 1267, 1277, 893 |
| Force stop, WebView HTTP cache deleted | 1741, 2624, 3110 |
| Right after `adb install -r`, cache deleted | 3378 |

`am start -W` reported 590–1570 ms to the first frame. The ~20 s first load in #226 didn't reproduce. The
shell does nothing before `loadUrl` except create the WebView, so the likely cause is the emulator's first
WebView start after boot or install, or the network at the time. Nothing in the shell needed a fix. Still
open: a real phone, and Tailscale off the LAN.

## 7. Identity in the web app (#229)

The user agent ends in `ReelAndroidWeb/<versionName>`, plus ` (TV)` when the UI mode is television. Built
(#229, ARCHITECTURE A61): with the token, the web app signs in as client `android-web` named "Reel Web on
<model>" (or "Reel Web on Android" / "Android TV"), decide keeps `web_chrome`, TV mode turns on exactly when the
token ends in ` (TV)` (a phone whose model contains "TV" doesn't get it), and the Chromecast row says it isn't
available (WebView has no Cast sender). Plain Android TV, Fire TV, webOS and Tizen browsers are detected as before.
Not yet seen through the real APK; needs a CT 113 deploy.

## 8. Playback in WebView (#230)

First smoke check (2026-11-18, Android 15 emulator, WebView 124, live server): Resume on a library title
opens the web player, which loads 1920×1040 metadata at the resume point and then buffers; the emulator's
software decoders are slow, so this says nothing about phones yet. The full matrix (direct play, MKV remux,
HEVC 8/10-bit, AC3/EAC3, transcode, resume, text and image subtitles, IPTV VOD, Live TV server and direct
delivery, full-screen exit, background) is #230's job, recorded here with the exact failing cases.

## 9. Android TV (#231)

No Leanback launcher entry or banner until the D-pad check on the Android TV emulator passes. FEATURES.md
says "phone" until then.

## 10. Releases (#232)

GitHub Actions is unavailable for now (account billing), so release APKs for **both** Android apps are built
on the owner's Mac with one script (ARCHITECTURE A65):

```sh
scripts/android-release.sh web     [--version X.Y.Z] [--taildrop] [--github-release] [--dry-run]
scripts/android-release.sh compose [--version X.Y.Z] [--taildrop] [--github-release] [--dry-run]
```

- **Build.** `./gradlew --no-daemon lint testDebugUnitTest assembleRelease` with `REEL_VERSION_NAME`/`CODE`
  and the signing env vars that `app/build.gradle.kts` already reads. Release builds are R8-shrunk and get no
  baked-in server (`REEL_URL` is unset).
- **Version.** `--version X.Y.Z`, or the next patch after the app's newest release tag (fetched from
  origin). The code follows `clients/android/scripts/version-from-tag.sh`, which accepts both tag prefixes:
  `X*1_000_000 + Y*1_000 + Z`. The two apps have different packages, so their versions are independent.
- **Signing.** The Compose app's release key, `~/.reel-android/reel-release.jks` (alias `reel`), so a
  release installs over the previous one. The passwords come from `REEL_KEYSTORE_PASSWORD`/`REEL_KEY_PASSWORD`,
  the macOS Keychain (service `reel-android-keystore`, account `reel`) or a hidden prompt. They reach only
  `keytool` (`-storepass:env`) and Gradle, never argv, a file or the log. Without them the script stops before
  building. After the build, `apksigner` must show the keystore's certificate, and the package, versionCode
  and versionName are checked. A key whose certificate isn't the release certificate
  (`a48654…58ecb`, the one on `android-v0.1.0`–`0.3.0`) gets a warning, and `--github-release` refuses it.
- **Output.** `dist/ReelWeb-X.Y.Z.apk` or `dist/Reel-X.Y.Z.apk` (git-ignored) with a `.sha256` next to it.
- **Taildrop** (`--taildrop`, `--taildrop-target NAME`, default `rafiiiki`) runs the app bundle's CLI
  (`/Applications/Tailscale.app/Contents/MacOS/Tailscale file cp`). When the phone is offline Tailscale says
  "reportedly offline" and fails with 502; the script says so, prints the command to send it later, and
  exits 3.
- **GitHub release** (`--github-release`, opt-in) tags HEAD, pushes the tag and runs `gh release create`
  (REST, no Actions) with the APK and its `.sha256`. It needs a clean tree, HEAD on `origin/main` and a new
  tag. Tags: `android-web-vX.Y.Z` (title "Reel Web X.Y.Z") and `android-vX.Y.Z` (title "Android X.Y.Z", as
  before). Neither glob matches the other's tags, so `android-release.yml` (`android-v*`) never sees Reel Web.
- **Obtainium** (one app entry per APK, same repo; a private repo needs a GitHub token in Obtainium's
  settings): Reel Web filters release titles by `^Reel Web ` and APKs by `^ReelWeb-.*\.apk$`; the Compose app
  by `^Android ` and `^(Reel|reel-android)-[0-9].*\.apk$` (the workflow named its APKs `reel-android-X.Y.Z.apk`).
- `.github/workflows/android-release.yml` stays in the repo but isn't in use while Actions is unavailable.
- `--dry-run` builds for real but only prints the Taildrop and GitHub commands. Unit tests:
  `scripts/tests/android-release-test.sh` (stubs, a throwaway keystore, no network or build).

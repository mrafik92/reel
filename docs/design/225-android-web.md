# Design: Reel Web for Android, a WebView client (#225)

> **Status (2026-11-18).** The shell (#226) is built: server setup, an origin-scoped WebView, the TLS and
> unreachable-server screens, and CI. Back, insets and full screen (#227), recovery (#228), the web-side
> identity (#229), the playback check (#230), the TV check (#231) and releases (#232) are open. When a part
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
    ├── MainActivity       the WebView, Back, error panel, renderer-crash recovery, launcher shortcut
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
  are separate sign-ins, and switching back to one keeps its session.
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

Built in #226: Back walks the WebView history and leaves the app on the first page; the content view pads
for the status bar, navigation bar, cutout and keyboard (edge-to-edge is enforced at targetSdk 35); config
changes don't recreate the activity. Open for #227:

- Full screen (`onShowCustomView` / Fullscreen API): hide bars, allow landscape, restore on exit.
- Back while full screen exits full screen first, and open web menus or dialogs should close before history
  goes back.
- Chromium skips history entries created without a user gesture, so `canGoBack()` is false after purely
  scripted navigation. Real taps work (checked: Search → title → Back returns to `/search?q=…` with the
  same results). Web code that navigates without a gesture can make Back leave the app.
- Returning to Search refocuses the box and opens the keyboard on the phone. Scroll restoration on long
  result lists needs a check.
- `enableOnBackInvokedCallback` (predictive back) is not on yet.

## 6. Recovery (#228)

Built in #226: a main-frame load error or a 5xx shows *Can't reach Reel* / *Reel had a problem* with
**Retry** and **Change server**. Open for #228: an unroutable address leaves a blank page while Chromium
waits on its own connect timeout (minutes), so the shell needs its own load timeout; automatic retry when the
network returns; error classification tests.

## 7. Identity in the web app (#229)

The user agent ends in `ReelAndroidWeb/<versionName>`, plus ` (TV)` when the UI mode is television. Until
#229 the web app sees a normal Chrome on Android: Active sessions says "Chrome on Android" and decide uses
`web_chrome`. Note that `detectPlatform` already treats an Android UA containing `TV` as `android-tv`.

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

Not built yet. Plan: tag `android-web-v*` (doesn't match the Compose app's `android-v*`), same signing
secrets, an asset name Obtainium can filter on.

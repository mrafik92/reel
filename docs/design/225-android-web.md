# Design: Reel Web for Android, a WebView client (#225)

> **Status (2026-11-18).** The shell (#226) is built: server setup, an origin-scoped WebView, the TLS and
> unreachable-server screens, and CI. Back, insets and full screen (#227), the web-side identity (#229) and the
> Android TV check (#231, emulator only) are built. Recovery (#228), the playback check (#230) and releases (#232)
> are open. When a part ships, FEATURES.md and ARCHITECTURE.md describe it and this note keeps what's still open.

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

Built in #226: a main-frame load error or a 5xx shows *Can't reach Reel* / *Reel had a problem* with
**Retry** and **Change server**. Open for #228: an unroutable address leaves a blank page while Chromium
waits on its own connect timeout (minutes), so the shell needs its own load timeout; automatic retry when the
network returns; error classification tests.

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

**Result: passes on the emulator, with four fixes (below).** The APK is now listed on Android TV: the
`LEANBACK_LAUNCHER` category, `android.software.leanback` `required=false` (one APK for phones and TV,
ARCHITECTURE A65) and a 320×180 xhdpi banner from `gen-icons.ts` (dark, "Reel WEB"; the Compose app's banner
is crimson). `TvManifestTest` keeps Leanback and the touchscreen optional.

Checked 2026-11-18 on the `reel-tv` AVD (Android TV 14, 1920×1080 at xhdpi, WebView 113), with the shell
pointed at a dev build of the web app (`main` + #231; `/api` proxied to the live server). D-pad and remote
keys only, no taps. Recording (68 s, 720p): [reel-web-231-tv.mp4](https://github.com/mrafik92/reel/blob/231-recording/reel-web-231-tv.mp4)
on the throwaway branch `231-recording`: Home → Spirited Away → Resume → playback → Back → Back, then Search
typed on the on-screen keyboard → Alpha → Back → Back → Back leaves the app.

**What WebView delivers.** The page runs at 960×540 CSS px (device pixel ratio 2). TV mode scales with the
viewport (`html.tv` font size 1.25vw), so the 10-foot layout is the same as at 1920×1080. Every remote key
reaches the page as a `keydown`, and the shell maps nothing:

| Android key | `key` / `keyCode` in the page | Web app |
|---|---|---|
| DPAD arrows | `ArrowUp`… / 38… | spatial navigation |
| DPAD_CENTER, ENTER | `Enter` / 13 | activate |
| BACK | none: the shell sends `Escape` (§5) | Back order |
| MEDIA_PLAY_PAUSE | `MediaPlayPause` / 179 | toggle |
| MEDIA_PLAY, MEDIA_PAUSE | `MediaPlay`, `MediaPause` / **179** | play, pause (fixed, see below) |
| MEDIA_FAST_FORWARD, MEDIA_REWIND | `MediaFastForward` / 228, `MediaRewind` / 227 | +30 s, −10 s |
| MEDIA_STOP, NEXT, PREVIOUS | `MediaStop` / 178, `MediaTrackNext` / 176, `MediaTrackPrevious` / 177 | stop; next/previous unused |
| CHANNEL_UP, INFO, PROG_RED, digits | `ChannelUp` / 33, `Info` / 0, `ColorF0Red` / 0, `5` / 53 | Live TV zapping, info |

**Checked, passing.**
- First focus: opening from the launcher banner, from `am start` and after a cold boot, the WebView has focus
  and the page has a focused element (sign-in: Username; Home: the hero's Play), so the first arrow works
  without a touch. Retry on the error panel hands focus back to the page.
- Focus is visible on Home, Search, item detail, Library (Movies/Shows, genres, filters), IPTV VOD (series
  rows and detail), Live TV (groups, channel cards, preview) and the player.
- Search and Sign in with the Leanback on-screen keyboard: OK on a field opens it, keys type, its
  *next*/*search* key moves on or submits (Search then focuses the first result), and Back closes it first.
  On Sign in, moving focus into the next field with the D-pad opens the keyboard again; that's Android TV's
  usual behaviour for a focused text field. Credentials were filled through DevTools, never typed with adb.
- Back follows #227: player → item → Home; item → Search with the same query and focus on the opened card;
  sheets and the keyboard close first; Back on `/` leaves the app (the page stays in memory).
- Live TV with the hub idle: one channel tuned in the preview for about 10 s, Back left the page and the
  hub showed 0 of 1 connections again.
- The native setup screen and error panel work with the D-pad: input → Connect → *Save anyway*, Retry →
  *Change server*, each with a visible focus ring.

**Fixed in #231.**
1. Native buttons showed focus only as a slightly lighter grey. They now use pill styles with the web
   app's cream focus ring (`bg_button`, `bg_button_text`).
2. Connect was disabled during the server check, which moved D-pad focus into the address field. It
   stays enabled behind a guard flag, so focus stays on it.
3. Back on Home did nothing: TV mode called `exitApp()`, which is a no-op outside webOS and Tizen, and
   marked the key handled, so the shell never saw it come back unhandled. `exitApp()` now reports whether
   it exited (web, `@reel/ui`).
4. WebView sends MediaPlay and MediaPause with keyCode 179 (the toggle code), and `normalizeKey` trusts
   keyCodes first, so pressing Pause twice resumed playback. Those two now go by `key`. Also, long track
   labels ("AAC · 5.1", "EN SDH Auto") pushed the player's right-hand group over the transport buttons. In TV
   mode the group keeps its width. That overlap isn't WebView-specific (webOS and Tizen get the fix too).

Fixes 3 and 4 are web code, so they reach the APK with the next CT 113 deploy, as does TV mode itself
(#229). The live build (`1a39b38`) doesn't recognise the shell's ` (TV)` token, so until that deploy the APK
shows the pointer layout on a TV.

**Open.**
- #243: Live TV opens with focus still on the side rail, and switching Movies/Shows to the IPTV source
  drops focus to the page body. In both cases the next arrow press recovers.
- Playback on the emulator: H.264/AAC direct play works. An HEVC + E-AC3 title (Guardians of the Galaxy,
  audio transcode) stalled with no segment after 25 s. That's #230's matrix; the emulator has only software
  decoders.
- No real Android TV device has been checked, and no physical remote (only `adb shell input keyevent`).

## 10. Releases (#232)

Not built yet. Plan: tag `android-web-v*` (doesn't match the Compose app's `android-v*`), same signing
secrets, an asset name Obtainium can filter on.

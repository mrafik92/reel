# Design: Reel Web for Android, a WebView client (#225)

> **Status (2026-11-18).** The shell (#226) is built: server setup, an origin-scoped WebView, the TLS and
> unreachable-server screens, and CI. Back, insets and full screen (#227), the web-side identity (#229),
> recovery (#228: load timeout, plain error messages, automatic retry) and the Android TV check (#231, emulator
> only) are built. Release builds (#232) are built locally with `scripts/android-release.sh`. The playback
> matrix (#230, §8) is recorded on the emulator and waits for the owner's phone. When a part ships, FEATURES.md
> and ARCHITECTURE.md describe it and this note keeps what's still open.

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
    ├── MediaKeys          the headset button → play/pause for the page (#230)
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

**Media keys** (#230, A68): WebView turns Android's media keys (play, pause, play/pause, fast-forward,
rewind) into the DOM keys the web player already handles, except the wired-headset button
(`KEYCODE_HEADSETHOOK`), which it maps to no DOM key. `MediaKeys` sends that one to the page as play/pause
through the normal key path, so it reaches the WebView inline and WebView's own view in full screen. The keys
work only while the app is in the foreground. There's no Android MediaSession, so there are no lock-screen
controls.

Still open: the setup screen's keyboard (native, unchanged) on a small phone, and a real phone.

**App feel** (#250): WebView's page-like defaults are off: the stretch overscroll (Android 12+ stretched the
whole WebView, the fixed bottom bar included), the native scrollbar (drawn over the bottom bar), pinch and
double-tap zoom (`setSupportZoom(false)`), and long-press text selection outside text fields (the long-press is
consumed, without a haptic). The web app's own scroll containers and inputs are unchanged. Checked on the phone
emulator: no scrollbar on load, no stretch when over-swiping at the end of a long title page. A real two-finger
pinch can't be sent on the emulator, so zoom is checked on the phone only.

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

Checked on 2026-11-18 on the **Android 15 phone emulator** (`sdk_gphone64_arm64`, WebView 124.0.6367.219, software
decoders), with the debug APK and the current web app from `main`. The web app ran from a dev build that proxied
`/api` to the live server, since CT 113 still serves an older web build. The background rows used a production
build (`vite preview`), because Vite's dev client reloads the page when it reconnects. All media came from the
live server's library and IPTV provider.

**How to read the times.** *First frame* is the web player's development-only `[ReelStartup]` mark (A58): the
time from the tap to the first painted frame. Live TV has no such mark, so those rows were timed by polling the
`<video>` until it played. The emulator decodes in software, so slow startup on 1080p and HEVC is **not a
phone result**. Every playing row needs a repeat on the phone (checklist at the end).

### Library and IPTV VOD

| Case | Asset (title, file id) | Container · video · audio | Play method (decide) | Result | First frame | Error or console |
|---|---|---|---|---|---|---|
| H.264/AAC MP4 direct play | Alpha (item 7, file 7) | MP4 · H.264 8-bit 1920×800 · AAC 2.0, external SRT | `direct_play`, subtitle as sidecar VTT | ✅ plays | 8.1 s | – |
| MKV remux to HLS | After Impact (item 1, file 1) | MKV · H.264 8-bit 1920×816 · AAC 2.0, 6 SRT | `remux` (video and audio copied): "container mkv not supported" | ✅ plays, multi-variant HLS through hls.js | 3.8 s | – |
| Resume from a saved position | After Impact (item 1, file 1), saved at 2 % | as above | `remux` | ✅ starts at the saved position (about 2:00) | 3.8 s | – |
| Text subtitles | After Impact (item 1, file 1), English SDH | embedded SRT | sidecar VTT, drawn by the player | ✅ cues on screen, also in full screen | – | – |
| HEVC 8-bit | Guardians of the Galaxy Vol. 2 (item 55, file 55) | MKV · HEVC Main 8-bit 1920×808 · AAC 5.1 | `remux` | ✅ plays | 16.7 s, 42 of the first 160 frames dropped | – |
| HEVC 10-bit with AC3 | Ant-Man and the Wasp (item 8, file 8) | MKV · HEVC Main 10 1920×804 · AC3 5.1 | `audio_transcode` (video copied, AC3 → AAC 5.1): "audio codec ac3 not supported" | ❌ **never starts**: endless spinner, the server reaps the session after about 60 s | – | `[player] stalled at 0.0 s: no segment arrived since the player was (re)started 26 s ago`. The master lists only `hvc1.2.4.*` (Main 10) rungs, the transcoded 480p/720p/1080p ones included, and this WebView's `MediaSource.isTypeSupported` rejects them (`hvc1.1.6.*` Main is accepted). The VOD profile sends `max_bit_depth: 10` for HEVC without probing it → **#245** |
| E-AC3 track | The Avengers (item 119, file 119) | MKV · H.264 8-bit 1920×1080 · E-AC3 5.1 | `audio_transcode` (E-AC3 → AAC 5.1 384 kbps) | ✅ plays (ABR starts on the 480p rung) | 12.5 s | WebView decodes neither AC3 nor E-AC3 (`ac-3` and `ec-3` are unsupported), so these tracks are always transcoded |
| Image subtitles and a transcode | The Avengers (item 119), English PGS picked in the player | embedded PGS | `transcode`: H.264 1080p 8 Mbps with burn-in, AAC 5.1 | ✅ the picture comes back with burned-in subtitles, ⚠ then a request storm | 7.3 s after the switch | hls.js re-requested the 480p `init.mp4` and segment 12 about every 100 ms (~250 aborted requests in 30 s) until the server answered **429** → **#246** |
| Quality switch | After Impact (item 1): Original → 480p, then 480p → 720p | as above | new decide, then a multi-variant session | ✅ both pins show within 10 s | – | – |
| Quality pin after the storm | The Avengers (item 119): Auto → 1080p | as above | `transcode`, multi-variant | ❌ after 90 s still "Quality · 1080p, playing 480p" | – | → **#246** |
| AV1 10-bit (extra) | Colony (item 29, file 29) | MKV · AV1 10-bit 1920×804 · TrueHD 7.1 | `audio_transcode` (TrueHD → AAC 5.1) | ✅ plays (dav1d software decoder) | 13.8 s | – |
| IPTV VOD movie | Madden (2026) (VOD movie 1487252) | MKV · video copied · AAC 2.0 | `remux` | ✅ plays. Provider slot 1/1 while playing, 0/1 right after Back | 9.9 s | – |
| IPTV VOD episode | The Remarried Empress S1 E2 (VOD episode 581, series 166172) | MKV · video copied · AAC 2.0 | `remux` | ✅ plays | 8.3 s | – |

### Live TV

Before each run the hub (`GET /api/v1/admin/iptv/hub`) showed 0 of 1 connections. Each run was short, and the hub was
back at 0 afterwards.

| Case | Channel | Delivery | Result | First frame | Error or console |
|---|---|---|---|---|---|
| Tune | Action 1 (11147) | server: HLS, video and audio copied | ✅ plays, hub 1/1 | about 3.5 s | – |
| Zap | Action 1 → Action 2 (11148) | server | ❌ "This channel isn't sending any picture" | – | The tune returned **503** `the channel did not deliver a valid stream` after about 16 s. The provider's channel is down, not the WebView. |
| Zap | Action 2 → Action 1 | server | ✅ plays | 3.2 s | – |
| Direct delivery | Action 1 | `direct` (allowed for this account: `/me` → `direct_live`) | ⚠ falls back to the server by itself, with a toast | 11.1 s in total | The server hands out the provider's raw MPEG-TS stream, which neither `<video>` nor hls.js can play in a browser (`ERR_ABORTED`). After about 2 s the page retunes through the server ("Couldn't play directly from the provider on this device — using the server instead."). That's how every browser behaves, not a WebView bug |
| Back | Action 1 | server | ✅ `DELETE /iptv/sessions/…` 204 right away, hub 0/1 | – | – |
| App exit (swiped away in Recents) | Action 1 | server | ✅ hub 0/1 about 17 s after the process died | – | – |
| Background (Home), then return | Action 1 | server | ✅ released, ⚠ return fails once | after Try again, 3.7 s | Android blocks the network of a cached app, so the page's own 30 s "park" `DELETE` fails, but the server reaps the session about 35 s after the app went to the background. On return the page retunes at once, gets `ERR_CONNECTION_TIMED_OUT` before the network is back and shows *Can't reach Reel*. **Try again** works → **#248** |

### Full screen, keys and lifecycle

| Case | Result |
|---|---|
| Enter with the player's button | ✅ landscape, bars hidden, keeps playing |
| Exit with Back / with the player's button | ✅ both: portrait and bars return, playback continues |
| Rotation while full screen (device turned 4 × 90°) | ✅ stays landscape and flips between the two landscape sides, playback continues |
| End of a title in full screen | ✅ stays full screen with the end controls (the web player's design, §5) |
| Next episode | The web player has **no autoplay**, only a *Next* button in the controls. Tapping it in full screen **leaves full screen** and the phone turns back to portrait: the Player remounts and its cleanup exits full screen → **#247** |
| Media keys (play, pause, play/pause, fast-forward) | ✅ inline and full screen. WebView turns them into the DOM keys the web app handles |
| Wired-headset button (`KEYCODE_HEADSETHOOK`) | ❌ before, because WebView maps it to no DOM key. **Fixed in the shell** (`MediaKeys`, A68): it reaches the page as play/pause, inline and in full screen |
| Screen timeout | ✅ the window holds `KEEP_SCREEN_ON` while video plays, inline and in full screen |
| Audio focus | Chromium takes `AUDIOFOCUS_GAIN` when playback starts. It still held it while paused in the background. Losing focus (a call, another player) not tested: phone checklist |
| App to background mid-playback (library, production build) | ✅ Chromium pauses the video at once (`visibilityState` hidden). The server keeps the session (no pings, still there after 2 min). On return **playback resumes by itself** at the same position, and progress reports start again |
| App exit (swiped away) during IPTV VOD | ✅ eventually: no stop is sent, and the server's idle reaper frees the provider slot about 60–70 s later. A library session is simply left to the reaper |

### Shell changes and filed issues

- Shell fix in this ticket: the headset button (above). No lifecycle, full-screen or audio-focus bug turned up in
  the shell.
- Web and server issues (milestone M5): **#245** HEVC 10-bit profile and silent stall · **#246** hls.js request storm
  and an ignored pin · **#247** Next leaves full screen · **#248** Live retune after returning from the
  background.
- Test-environment notes (not product bugs): another thread's dev server held `127.0.0.1:5182`, which is what the
  emulator's `10.0.2.2` reaches, so the shell pointed at the Mac's LAN address instead (it also answered "doesn't
  look like a Reel server" during setup). `adb forward --remove-all` from parallel threads drops the WebView
  DevTools forward, so re-forward before every CDP call.

### Phone checklist (owner, #196-style)

Repeat on the phone with the same titles. They're named by item id, so a library rescan doesn't change them:

1. Alpha (7) direct play, After Impact (1) remux with resume and subtitles: note the first frame.
2. Guardians Vol. 2 (55) HEVC 8-bit and **Ant-Man and the Wasp (8) HEVC 10-bit**. Most phones decode Main 10 in
   hardware, so #245 may not show there. If it plays, note the first frame. If it spins, it's #245.
3. The Avengers (119): pick the English PGS track with `chrome://inspect` open on the debug build, and watch the
   Network tab for the same 480p segment repeating. Then pin 1080p (#246).
4. Full screen: enter, rotate the phone, leave with Back and with the button. Play an episode to the end, then
   *Next* (#247).
5. A wired or Bluetooth headset: play/pause from its button. A phone call or another player during playback (audio focus).
6. Home mid-playback, back after a minute: library and Live TV (#248). Swipe the app away during Live TV and check the
   hub.
7. Live TV tune and zap on two working channels.

### Recommendation

The matrix doesn't justify a Media3 handoff yet. Every failure on the emulator is a web-player or server bug
with a small fix:

- #245 is a one-line profile fix (use the probed `bitDepth`) plus an error when no variant is playable.
- #246 is hls.js abandon and back-off tuning.
- #247 keeps full screen across *Next*.
- #248 retries the retune once.

Server-delivered Live TV, IPTV VOD, remux, audio transcode, burn-in and AV1 all play in WebView.

A handoff would be worth it only if the phone check shows one of these:

- 1080p remux or HEVC playback that's too slow or drops frames on real hardware.
- Something only a native player can do: background audio with lock-screen controls (an Android MediaSession),
  picture-in-picture, AC3/E-AC3 passthrough instead of an AAC transcode, HDR or Dolby Vision output, or direct
  Live delivery of the provider's raw MPEG-TS (WebView can't play it, so direct delivery always falls back to the
  server).

The handoff would be **large (L)**. The shell has no JavaScript bridge (§4), so the page would need a new path
to say "play this item at this position" (a bridge or an app link). The native side would need decide, progress,
resume, tracks, quality and the IPTV slot handling: in effect the Compose app's Media3 player (#190, #191) moved
into this APK or launched from it. If one of those native-only needs is what's missing, launching the Compose
app's player is the cheaper route.

## 9. Android TV (#231)

**Result: passes on the emulator, with four fixes (below).** The APK is now listed on Android TV: the
`LEANBACK_LAUNCHER` category, `android.software.leanback` `required=false` (one APK for phones and TV,
ARCHITECTURE A67) and a 320×180 xhdpi banner from `gen-icons.ts` (dark, "Reel WEB"; the Compose app's banner
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

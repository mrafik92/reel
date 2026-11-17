# Design: native Android app for TV and phone (#29)

> **Status: proposed 2026-11-17.** Built so far: the project skeleton (#185) and connect + sign-in (#186) and the app shell (#187), and the VOD player (#190, in review). When a part ships,
> FEATURES.md and ARCHITECTURE.md describe it and this note is trimmed to what's still open.

Epic #29 · milestone **M5 – Mobile** · sub-issues #185–#196.

## 1. The decision in one paragraph

Reel gets a **native Android app**: Kotlin, Jetpack Compose and Media3/ExoPlayer, in `clients/android/`.
**One APK** runs on phones, tablets and Android TV. It has a launcher and a Leanback-launcher entry and
adapts at runtime: a bottom bar and touch on phones, a nav rail and D-pad focus on TV. The app talks to the
existing `/api/v1` with a bearer token, like the web app does. Playback asks `/playback/{file}/decide` with the
server's existing **`android_exoplayer`** preset. Live TV tunes in **`passthrough`** mode: ExoPlayer plays the
provider's MPEG-TS as is, as webOS and Tizen do. **No server change is needed for the first release.**

## 2. Why native, not a WebView shell

The webOS and Tizen apps wrap the SolidJS UI and only swap the player. Android was chosen to go native
(owner's decision, 2026-11-17), as SPEC.md §9 planned:

- **ExoPlayer is the point.** It direct-plays HEVC, HDR10/HLG/Dolby Vision, AC3/EAC3/TrueHD passthrough,
  PGS/VobSub and MKV, which browsers can't. Most of the library then plays without a transcode (the N100's
  scarcest resource, see #102 and #113).
- **TV input and performance.** A Compose UI with real focus handling beats a WebView on cheap Android TV
  sticks: no Chromium 68-era constraints and no spatial-navigation library.
- **Trade-off:** the UI is written twice (SolidJS and Compose), so features land on Android later than on the
  web. The app stays deliberately smaller than the web: watching, not administration (see §8).

## 3. Shape of the code

```
clients/android/                     standalone Gradle project (not part of the bun workspaces)
  gradle/libs.versions.toml          every dependency version in one place
  app/src/main/java/app/reel/android/
    data/        API models (kotlinx.serialization), ReelApi (OkHttp), SessionStore (DataStore)
    ui/          theme, shell (nav), screens: server, login, home, library, item, search, live, settings
    ui/tv/       focus helpers: focusable cards, initial focus, rows that keep focus position
    player/      ReelPlayer (Media3 wrapper), VOD + live controllers, track/quality menus
```

| Choice | Picked | Why |
|---|---|---|
| UI | Compose + Material 3; TV focus with plain `focusable`/`clickable` modifiers | One toolkit for phone and TV. `androidx.tv:tv-material` isn't used (#187, ARCHITECTURE A51) |
| HTTP / JSON | OkHttp + kotlinx.serialization, hand-written models | Small API surface; no codegen step. A unit test parses the OpenAPI examples, so the models can't drift silently |
| Images | Coil (OkHttp-backed) | The server returns signed, resized image URLs; no auth header needed |
| Storage | DataStore (app-private): server URL, token, device id | `allowBackup=false`, so the token isn't copied into cloud backups |
| Player | Media3 ExoPlayer + `media3-ui` `PlayerView` | Built-in D-pad and touch controls; HLS + progressive + MPEG-TS |
| Min SDK | 24 (Android 7) | Covers older Android TV sticks |
| Package id | `app.reel.android` (`.debug` suffix for debug builds) | Debug and release can be installed side by side |

## 4. How the app uses the API

| Need | Endpoint | Notes |
|---|---|---|
| Pick a server | `GET /health`, `GET /setup` | Address typed once (LAN, Tailscale or `https://reel.rafikii.co`); `REEL_URL` at build time can preset it |
| Sign in | `POST /auth/login` with `client: "android"`, a stable `device_id`, `device_name` = device model | Signing in again from the same install replaces its session. Later: TV device-code login (#147) |
| Who am I | `GET /me` | Permissions hide Live TV / VOD the user can't use |
| Browse | `/home`, `/libraries`, `/libraries/{id}/items` (keyset cursor), `/items/{id}`, `/items/{id}/children`, `/search/all` | |
| Play | `POST /playback/{file}/decide` `{preset: "android_exoplayer", device_id, item_id, start_s}` | `url` is `/direct/{id}` or an HLS master; both are signed |
| Progress | `POST /playback/{sid}/progress` every 10 s, and on pause / stop | A `404` means the session was reaped: decide again at the playhead (same as web #95) |
| Stop | `DELETE /playback/{sid}` | Also on app background / process death (best effort) |
| Live | `POST /iptv/channels/{id}/tune` `{mode: "passthrough", preset}` → `stream.ts` | `429` all slots busy, admin `409` + `takeover`. `DELETE /iptv/sessions/{id}` on leave |
| Guide | `/iptv/channels`, `/iptv/groups`, `/iptv/now`, `/iptv/epg` | |

Rules:

- **The bearer token goes only to the configured server's origin.** Media URLs are signed, so ExoPlayer and
  Coil don't need the token at all. An OkHttp interceptor adds it to API calls only.
- Error classes match the web (#90): offline, timeout, unreachable, 401 (back to sign-in), 429/409 (busy).
- Sending `device_id` with decide lets the server's "newest wins" rules see the TV and the phone as different
  devices.

## 5. Player

- One `ReelPlayer` wraps ExoPlayer. VOD and Live use different controllers on top of it.
- **VOD:** decide → `MediaItem` (HLS mime type when the URL is `.m3u8`), sidecar WebVTT subtitles from
  `subtitles[].url`, start at the resume point (the HLS playlist covers the whole timeline, so positions are
  absolute). If the resume point is within 5 s of the end, it starts from 0. Audio and subtitle changes
  that need a different server decision call decide again at the playhead. Tracks that ExoPlayer can switch
  itself (direct play) switch locally.
- **Remote keys:** play/pause, ±10 s / ±30 s, media keys and Back. On TV, the controls hide after 3 s.
- **Live:** tune → play `stream.ts`. Channel up/down (and D-pad up/down while the controls are hidden) zaps,
  digits type a channel number, and an overlay shows the channel and now/next. When ExoPlayer fails, the app
  re-tunes once a minute at most, like the web tuner (#93).
- **Lifecycle:** pause on `onStop`; stop the session when the player screen leaves. No background audio or
  picture-in-picture in v1.

## 6. TV and phone UX

- `isTv` = `UiModeManager.currentModeType == UI_MODE_TYPE_TELEVISION`. It picks the layout, not the feature set.
- **TV:** left nav rail, 10-foot type scale, focused cards scale up with a cream outline, the first card gets
  focus on entry, and focus is restored when going Back. No touch assumptions.
- **Phone:** bottom bar (Home, Search, Live TV, Library, Settings), edge-to-edge, the player is landscape and
  immersive.
- Palette and type follow `packages/ui/src/styles/ui.css` (crimson + cream, #177). Icons come from the same
  `logo.svg` through `bun run icons` (launcher mipmaps, adaptive icon, 320×180 TV banner).

## 7. Build, CI, distribution

- **CI:** an `android` job in `ci.yml`, run when `clients/android/**` changes. It uses JDK 17 and the Gradle
  wrapper to run `lint`, `testDebugUnitTest` and `assembleDebug`, and uploads the debug APK as an artifact.
- **Release (#195):** on an `android-vX.Y.Z` tag, `android-release.yml` builds a signed, R8-shrunk release APK
  (versionCode `X*1_000_000 + Y*1_000 + Z`) and attaches it to a GitHub release. The PR `android` job also builds
  an unsigned `assembleRelease`, so R8 failures show up before a tag.
  The keystore and its passwords live only in Actions secrets and the owner's password manager. Install it
  by sideloading (`adb install`, or the *Downloader* app on TV). No Play Store for now.
- **Local:** JDK 21 + Android command-line tools (Homebrew). Use emulators for an Android TV (1080p) and a
  phone. `clients/android/README.md` has the commands.

## 8. Scope

**v1 (this epic):** server address, sign-in, home, libraries, item detail (movie / show / season / episode),
search, VOD player, Live TV with zapping and a guide, IPTV VOD catalog, playback settings, About, signed
release APK.

**Not in v1 (Ideas, separate tickets if wanted):** administration screens, requests and Discover (web only
for now), offline downloads, Chromecast sender, picture-in-picture, background audio, Android Auto, Play
Store listing, the `/events` WebSocket (the app refreshes on resume instead).

## 9. Tickets and order

| # | Ticket | Size | Depends on |
|---|---|---|---|
| #185 | Bootstrap: Gradle project, Compose skeleton, theme, icons + TV banner, CI job | M | – |
| #186 | API client, server address, sign-in, session storage, error classes | M | #185 |
| #187 | App shell: navigation, TV nav rail / phone bottom bar, D-pad focus kit, image loading | M | #186 |
| #188 | Home, libraries (paged grid, sort/filter), item detail, seasons and episodes, mark played | L | #187 |
| #189 | Search | S | #187 |
| #190 | VOD player: decide, resume, progress, tracks, quality, remote keys, next episode | L | #187 |
| #191 | Live TV: channel list, groups, favorites, now/next, passthrough tune, zapping, busy/takeover | L | #190 (shares `ReelPlayer`) |
| #192 | TV guide (EPG grid) | M | #191 |
| #193 | IPTV VOD: movies and series catalog, play | M | #190 |
| #194 | Settings: playback preferences, signed-in devices, sign out, About | S | #187 |
| #195 | Signed release APK on tag + install docs | M | #185 |
| #196 | Verify on a real Android TV and phone (direct play HEVC/HDR, zap time, D-pad) | – | #188–#191 |

Waves (no more than two threads writing app code at once, so the shell doesn't conflict):

1. **#185** alone.
2. **#186 → #187** in one thread (they share the shell and the session).
3. **#188 ∥ #190**, then **#189, #194** (small).
4. **#191 → #192**, with **#193 ∥ #195**.
5. **#196** on real devices (owner).

## 10. Open questions

- Which Android TV device(s) will verify it (#196)? That decides the codec and HDR checks.
- TV sign-in: typing a password with a remote is slow. Ship v1 with username and password, and switch to the
  device code once #147 lands?
- Keep `app.reel.android` as the package id? It can't change once a release is installed.

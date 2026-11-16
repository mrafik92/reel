# Features

Status key: **✅ live**: deployed on CT 113 and checked against real data. **🧪 built**: implemented and
tested in CI, but not yet exercised on real hardware or with real data. **⏳ partial**: some of it works and the
gaps are listed in [STATUS.md](STATUS.md).

## Library & metadata

| Feature | Status | Notes / where |
|---|---|---|
| Library scanning (movies, shows, anime) | ✅ | `crates/scanner`: parses filenames for movies, TV, specials and date-based names, probes with ffprobe, rescans incrementally (path+size+mtime), updates live through a file watcher |
| Rename/move detection | ✅ | Matches a vanished file to a new one by size, duration and stream fingerprint, so watch state survives renames |
| Multi-episode files (`S01E01E02`) | ✅ | File↔episode link table; finishing the file marks every episode in it |
| TMDB metadata | ✅ | `crates/metadata`: matching, posters/backdrops, cast & crew, collections, TV seasons/episodes |
| Local `.nfo` + artwork | ✅ | Works without a TMDB key; local files take precedence over TMDB; fields can be locked per item. Artwork symlinked outside the library roots, or not a real JPEG/PNG/WebP, is refused (🧪 fix for #10, pending deploy) |
| Image cache & resizing | ✅ | Resized on demand (Lanczos3), cached on disk, ETag + long cache headers. Downloads of TMDB images, channel logos and VOD artwork are size- and time-capped while streaming, shared between concurrent requests, and the caches stay under a disk budget (4 GiB TMDB, 1 GiB IPTV by default; least recently used files go first) (🧪 #9, pending deploy) |
| Radarr/Sonarr webhooks → targeted rescan | ✅ | When an import completes, Reel rescans just that path |

## Browsing & discovery

| Feature | Status | Notes |
|---|---|---|
| Home (continue watching, recently added, picker lanes) | ✅ | |
| Movie picker | ✅ | Lanes ("Because you watched…", top unwatched, under 100 min, new, rediscover, per genre), genre chips, filters (genre, decade, rating, runtime, unwatched), sort by rating/random, **Surprise me** |
| Global search | ✅ | `/` or ⌘K. Covers library titles and cast names, episodes, people, TMDB titles not in the library, and IPTV movies/series |
| Actor / person pages | ✅ | Bio and full filmography with availability badges; **Add** with a live progress ring; **Play** for titles you own. Filmography sort (🧪 #141, pending deploy): **Newest** (default), **Oldest**, **Top rated** (TMDB rating; titles under 50 votes go after the rest, unrated last; cards show ★ rating) and **Popular** (most TMDB votes), combined with the type and library filters, remembered per browser |
| Ratings & reviews | ⏳ | TMDB rating and reviews ✅. IMDb/RT/Metacritic scores need an OMDb key (not configured yet). Optional Trakt comments. Links out to IMDb, Rotten Tomatoes and Letterboxd (Letterboxd has no API) |

## Requests & downloads (built-in "Seerr")

| Feature | Status | Notes |
|---|---|---|
| Search TMDB → **Add** → Radarr/Sonarr | ✅ | One click for admins and auto-approve users. Others go through approval and per-user quotas |
| Live progress ring | ✅ | States: searching → queued → downloading % → importing → available (stalled / failed / not found shown too). Pushed over WebSocket |
| Downloads page | ✅ | The whole Radarr and Sonarr queue, including items not started from Reel, with reasons such as "qBittorrent is downloading metadata" |
| qBittorrent / Transmission adapters | ✅ | Speed, ETA, peers, client state. Supports qBittorrent 5's `QBT_SID_<port>` cookie |
| Notifications | 🧪 | Discord, Telegram, generic webhook |

## Playback

| Feature | Status | Notes |
|---|---|---|
| Playback decision engine | ✅ | Chooses direct play, remux, audio-only transcode or full transcode from the device profile and the file's streams. Reasons appear in Admin → Now playing |
| Hardware transcoding (VAAPI on the N100) | ✅ | Full GPU pipeline (decode → scale_vaapi → h264_vaapi). Measured at 8.3× realtime for 1080p→720p. Falls back to software per attempt |
| Keyframe-accurate HLS for remux | ✅ | Reads the keyframe index from MKV Cues / MP4 `stss` (kilobytes, not the whole file). Segments are validated before serving, so seek/resume stays stable |
| Compressed HLS playlists | 🧪 | Every `.m3u8` (VOD master and media playlists, live, IPTV VOD) goes out brotli or gzip when the client sends `Accept-Encoding`. A 2.5 h film's signed media playlist: 216 KB → 6.5 KB gzip / 2.8 KB br, so hls.js reaches its first segment request sooner on slow links. Segments, direct play and other media are never compressed and keep `Range` (#112) |
| Adaptive quality (server) | ✅ | `quality` = auto/original/1080p/720p/480p plus a `max_bitrate_kbps` hint. Ladder: 8/4/2 Mbps, never upscales |
| Adaptive quality (player: bandwidth estimate, auto step-down, quality menu) | ⏳ | Not built yet (#87). For now there's a per-user bitrate cap (Settings → Users). A link slower than a file's peak bitrate still stalls (the player no longer stops, but it can't play faster than the link) |
| Subtitles | ✅ | Reel's own overlay on every platform. Text subtitles → WebVTT. Image subtitles are burned in. Human-readable labels such as "English · SDH" |
| Automatic audio/subtitle selection | ✅ | Settings → Playback (languages, Off / Forced only / Automatic / Always, SDH). The server picks the tracks and explains why |
| Playback recovery (web) | 🧪 | hls.js plays HLS in every MSE browser, Chrome included (Chrome's own HLS player gave up on the first bad segment). VOD keeps ~90 s buffered with longer segment timeouts, so bitrate peaks don't drain it. Network errors, MSE/decoder errors and stalls of 25 s (live: 12 s) are retried by themselves (reload at the playhead or the live edge, `recoverMediaError`, `swapAudioCodec`, rebuild) with a **"Reconnecting…"** overlay. After 8 failed attempts the player reopens the session (live: re-tunes) once, at most once a minute, before it shows the error card (#91, #93). Safari keeps its native HLS player; TV apps use their own players. **Pause is kept** (#100): a recovery or an automatic reopen never starts a paused video. **Long pauses** (#95): a paused player keeps pinging, and the server keeps a paused converted session for 30 min instead of 60 s. When the server has ended the session anyway (reaped, restarted), the player opens a new one at the same position, right away if it was playing and on the next play if paused. A stall's error now says whether a segment request is stuck in flight or none was made (#96) |
| Player UI | ✅ | Redesigned controls: centred transport, scrubber with a time tooltip, labelled actions, volume/speed/PiP, keyboard shortcuts, D-pad seek mode |
| Player menus and toasts in full screen (web) | 🧪 | Speed, audio, subtitle and "Play on…" sheets, and toasts, mount inside the full-screen player, so they open over the video and close back to it without leaving full screen. Before, they opened invisibly behind it (#94) |
| Watch state | ✅ | Resume, continue watching, played toggles cascade to seasons and shows |

## Live TV & IPTV

| Feature | Status | Notes |
|---|---|---|
| Xtream Codes + M3U ingest | ✅ | The user's provider: 11,950 channels in 175 groups |
| Provider login hidden in admin settings | 🧪 | Admin → Live TV never shows the provider login: the playlist and guide URLs come back with `***` in place of credentials and the Xtream username as `ab••••` (#15). To change the login, type it again; a masked link can't be saved or tested as it is, and moving an Xtream provider to another server needs the username and password again |
| Shared upstream + keyframe (GOP) cache | ✅ | One provider connection per channel no matter how many viewers watch it. New viewers start at the cached keyframe almost instantly |
| Single-connection plan handling | ✅ | Your own older stream gives up the slot automatically (newest wins). Others get a friendly 429, and admins get a **Take over** button |
| Direct (local) Live TV playback | 🧪 | Off by default (`REEL_ALLOW_DIRECT_LIVE`). When on, **Settings → Playback → Live TV on this device** (Playback is the first entry in the Settings menu) lets a device play channels straight from the provider instead of through Reel. The device's choice is stored locally. Reel only holds the provider connection slot (so the budget still works) and the device sends a heartbeat every 15 s. If a device can't play the provider's stream (browsers usually can't: CORS, mixed content, MPEG-TS), that channel falls back to the server. Trade-offs: no shared upstream or keyframe cache (slower zapping), and the provider login is sent to the device |
| Live HLS window | 🧪 | Browser live streams use ~2 s fMP4 segments (the first is 1 s for a quick tune) in a 30 s playlist, and a segment stays servable ~20 s after it leaves the playlist, so a viewer who falls behind isn't hit by 404s. LATM AAC audio (stream type 0x11) is re-encoded to AAC, other AAC is copied (#93) |
| Live TV UI | ✅ | Player + two horizontal rows. Full-screen **group browser** (smart sections, Arabic-aware search, pin/hide). No auto-tune |
| TV guide (EPG) | ✅ | The provider has no guide data, so Reel combines several external XMLTV guides (epgshare01: AE, beIN, Al Jazeera, UK, ZA, FR) matched by name. ~1,436 channels, ~54k programmes |
| Guide grid | ✅ | Virtualized channels × time grid |
| IPTV movies & series (VOD) | ✅ catalog / ✅ direct play / 🧪 remux | 41,268 movies and 15,109 series imported. **My library · IPTV** switch on Movies/Shows; detail pages; "Play now (IPTV)" in Discover. Playback goes through the internal upstream proxy with the source's User-Agent, and shares the provider's connection budget with live TV (a stop or takeover also closes a paused direct-play stream). Direct play verified on the real provider (2026-11-11, #3). A remux (video copy) serves a full, seekable VOD playlist: the keyframe index is read from the MKV Cues / MP4 `stss` with a few `Range` requests through the proxy (#92, built and tested in CI, not deployed; containers without an index still play but can't be seeked). Remux/transcode is covered by CI with fake providers |
| Provider politeness | ✅ | Per-source rate limiter with backoff on the panel's 46x/429 anti-abuse codes |

## Clients & casting

| Feature | Status | Notes |
|---|---|---|
| Web app (desktop + 10-foot TV mode) | ✅ | SolidJS. D-pad spatial navigation. Targets Chromium 68 for old TVs |
| App icon: favicon, PWA manifest, TV launcher icons | 🧪 | One source SVG (`clients/packages/ui/assets/logo.svg`, a lime film reel with a strip of film running off it) → favicon SVG/ICO, apple-touch-icon, manifest 192/512 + maskable, webOS icon/largeIcon/splash, Tizen icon via `bun run icons`. `theme-color` + `manifest.webmanifest` in `index.html` (#142) |
| LG webOS / Samsung Tizen apps | 🧪 | Packaging, remote keys, native players (`<video>` / AVPlay). **Never installed on a real TV** |
| Casting: AirPlay, Chromecast | 🧪 | Sender UI with a "Playing on…" remote view. Cast URLs point at the LAN address. Needs real devices to verify |
| Casting: DLNA "Play on TV" | 🧪 | Server-side SOAP control; devices found by SSDP or added by IP. Waiting on TV IPs + a CT 113 firewall rule. Discovery can be turned off (`REEL_CAST_DISCOVERY=false`); a TV's description and control URLs must stay on the TV's own address, with capped bodies and no redirects (🧪 #12, pending deploy) |
| Android / iOS | — | Postponed |

## Sharing & administration

| Feature | Status | Notes |
|---|---|---|
| First-run setup, login, invites | ✅ | Argon2id passwords, opaque session tokens (30-day sliding window, 90-day absolute limit) |
| Per-user library access, limits, permissions | ✅ | Streams, transcodes (🧪 live TV video transcodes count too, #13), bitrate cap, request quotas. Flags for IPTV / VOD / requests / casting, off by default for invited users |
| Reconnects by itself (web) | 🧪 | API calls time out (20 s reads, 60 s writes) and reads are retried once. When the server answers again after a network failure, or the tab becomes visible / goes online, the current page is reloaded in place (not the player or Live TV), "Can't reach Reel" screens retry with backoff, and the events WebSocket reconnects (it is replaced after more than 60 s away). The server pings each socket every 25 s (#81) |
| Honest connection errors (web) | 🧪 | A failed request is classified: **cancelled** (page unloading, or the caller aborted it) shows nothing, **offline** (the browser has no network) says "You're offline", a **timeout** or **connection failure** says "Can't reach Reel". All say "Check your connection and try again" with a **Try again** button that the remote can reach. The `?mock=1` tip only appears in dev builds. On Live TV, a failed refresh keeps the channels already on screen and shows a toast instead of replacing the page (#90) |
| Signed media URLs | ✅ | HMAC with a per-user `sign_epoch` (revocable). Works in native players without auth headers |
| Admin settings | ✅ | Libraries, users & invites, Radarr/Sonarr, download clients, notifications, Live TV (providers, guides, channels, live hub), now playing, transcoding, **About** (server + web app versions) |
| Settings layout for everyone | 🧪 | One **Settings** page: section nav on the left, the section in the pane. **Playback** (`/settings/playback`) opens in the pane like the server sections and is highlighted in the nav; links from the player and item sheets land there. Every user gets the Settings entry in the rail and a nav with Playback and About; admins also get the server sections. On TV, left/right move between the nav and the pane (#88) |
| Audit log | ✅ | Admin actions, login failures (sampled), IPTV takeovers |

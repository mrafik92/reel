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
| Image cache & resizing | ✅ | Resized on demand (Lanczos3), cached on disk, ETag + long cache headers |
| Radarr/Sonarr webhooks → targeted rescan | ✅ | When an import completes, Reel rescans just that path |

## Browsing & discovery

| Feature | Status | Notes |
|---|---|---|
| Home (continue watching, recently added, picker lanes) | ✅ | |
| Movie picker | ✅ | Lanes ("Because you watched…", top unwatched, under 100 min, new, rediscover, per genre), genre chips, filters (genre, decade, rating, runtime, unwatched), sort by rating/random, **Surprise me** |
| Global search | ✅ | `/` or ⌘K. Covers library titles and cast names, episodes, people, TMDB titles not in the library, and IPTV movies/series |
| Actor / person pages | ✅ | Bio and full filmography with availability badges; **Add** with a live progress ring; **Play** for titles you own |
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
| Adaptive quality (server) | ✅ | `quality` = auto/original/1080p/720p/480p plus a `max_bitrate_kbps` hint. Ladder: 8/4/2 Mbps, never upscales |
| Adaptive quality (player: bandwidth estimate, auto step-down, quality menu) | ⏳ | Not built yet (the agent errored). For now there's a per-user bitrate cap (Settings → Users) |
| Subtitles | ✅ | Reel's own overlay on every platform. Text subtitles → WebVTT. Image subtitles are burned in. Human-readable labels such as "English · SDH" |
| Automatic audio/subtitle selection | ✅ | Settings → Playback (languages, Off / Forced only / Automatic / Always, SDH). The server picks the tracks and explains why |
| Player UI | ✅ | Redesigned controls: centred transport, scrubber with a time tooltip, labelled actions, volume/speed/PiP, keyboard shortcuts, D-pad seek mode |
| Watch state | ✅ | Resume, continue watching, played toggles cascade to seasons and shows |

## Live TV & IPTV

| Feature | Status | Notes |
|---|---|---|
| Xtream Codes + M3U ingest | ✅ | The user's provider: 11,950 channels in 175 groups |
| Shared upstream + keyframe (GOP) cache | ✅ | One provider connection per channel no matter how many viewers watch it. New viewers start at the cached keyframe almost instantly |
| Single-connection plan handling | ✅ | Your own older stream gives up the slot automatically (newest wins). Others get a friendly 429, and admins get a **Take over** button |
| Direct (local) Live TV playback | 🧪 | Off by default (`REEL_ALLOW_DIRECT_LIVE`). When on, **Settings → Playback → Live TV on this device** (Playback is the first entry in the Settings menu) lets a device play channels straight from the provider instead of through Reel. The device's choice is stored locally. Reel only holds the provider connection slot (so the budget still works) and the device sends a heartbeat every 15 s. If a device can't play the provider's stream (browsers usually can't: CORS, mixed content, MPEG-TS), that channel falls back to the server. Trade-offs: no shared upstream or keyframe cache (slower zapping), and the provider login is sent to the device |
| Live TV UI | ✅ | Player + two horizontal rows. Full-screen **group browser** (smart sections, Arabic-aware search, pin/hide). No auto-tune |
| TV guide (EPG) | ✅ | The provider has no guide data, so Reel combines several external XMLTV guides (epgshare01: AE, beIN, Al Jazeera, UK, ZA, FR) matched by name. ~1,436 channels, ~54k programmes |
| Guide grid | ✅ | Virtualized channels × time grid |
| IPTV movies & series (VOD) | ✅ catalog / 🧪 playback | 41,268 movies and 15,109 series imported. **My library · IPTV** switch on Movies/Shows; detail pages; "Play now (IPTV)" in Discover. Playback goes through the internal upstream proxy and hasn't been exercised on the real provider yet |
| Provider politeness | ✅ | Per-source rate limiter with backoff on the panel's 46x/429 anti-abuse codes |

## Clients & casting

| Feature | Status | Notes |
|---|---|---|
| Web app (desktop + 10-foot TV mode) | ✅ | SolidJS. D-pad spatial navigation. Targets Chromium 68 for old TVs |
| LG webOS / Samsung Tizen apps | 🧪 | Packaging, remote keys, native players (`<video>` / AVPlay). **Never installed on a real TV** |
| Casting: AirPlay, Chromecast | 🧪 | Sender UI with a "Playing on…" remote view. Cast URLs point at the LAN address. Needs real devices to verify |
| Casting: DLNA "Play on TV" | 🧪 | Server-side SOAP control; devices added by IP. Waiting on TV IPs + a CT 113 firewall rule |
| Android / iOS | — | Postponed |

## Sharing & administration

| Feature | Status | Notes |
|---|---|---|
| First-run setup, login, invites | ✅ | Argon2id passwords, opaque session tokens (30-day sliding window, 90-day absolute limit) |
| Per-user library access, limits, permissions | ✅ | Streams, transcodes, bitrate cap, request quotas. Flags for IPTV / VOD / requests / casting, off by default for invited users |
| Signed media URLs | ✅ | HMAC with a per-user `sign_epoch` (revocable). Works in native players without auth headers |
| Admin settings | ✅ | Libraries, users & invites, Radarr/Sonarr, download clients, notifications, Live TV (providers, guides, channels, live hub), now playing, transcoding, **About** (server + web app versions) |
| Audit log | ✅ | Admin actions, login failures (sampled), IPTV takeovers |

# Reel — Self-hosted media server (spec v0.1)

> Working name. A self-hosted Plex/Jellyfin alternative with built-in requests (Seerr-like)
> and a first-class, low-latency IPTV engine. No cloud dependency, no vendor account.

## 1. Goals / non-goals

**Goals**
- Fully local: no third-party auth, no cloud relay, no paywall. Works offline on LAN.
- Fast, clean UX on TVs (LG webOS, Samsung Tizen), web, Android; iOS later.
- Direct-play first; transcode only when the client truly can't decode.
- Built-in media requests wired to Sonarr/Radarr — no separate Jellyseerr.
- IPTV that zaps in < 1s and uses one upstream connection per channel regardless of viewer count.
- Safe to share with friends over the internet (invites, per-user limits, hardened auth).

**Non-goals (for now)**
- Photos (use Immich), music (maybe later), books, plugins marketplace.
- Cloud relay / NAT traversal service.
- Native ffmpeg bindings — ffmpeg/ffprobe are always subprocesses.

## 2. Architecture

```
            ┌──────────── clients ────────────┐
 web/PWA  webOS  Tizen  (SolidJS shared UI)   Android (Kotlin/Media3)   iOS (later)
            └──────────────┬──────────────────┘
                           │ HTTPS: REST (OpenAPI) + HLS/fMP4 + WebSocket (events)
┌──────────────────────────▼──────────────────────────────────────────────┐
│ reel-server (Rust, tokio + axum, single binary)                          │
│  api ─ auth ─ playback ─ iptv ─ requests ─ scanner ─ metadata ─ core(db) │
└───────┬──────────────┬───────────────┬───────────────┬──────────────────┘
     SQLite (WAL)   ffmpeg/ffprobe   TMDB/TVDB       Sonarr/Radarr, IPTV providers
                    (QSV/VAAPI/NVENC)
```

| Concern | Choice |
|---|---|
| Runtime / HTTP | tokio, axum, tower (rate limit, compression, tracing) |
| DB | SQLite via sqlx, WAL mode, migrations in `crates/core/migrations` |
| API contract | utoipa → `openapi.json` → generated TS SDK (`clients/packages/sdk`) |
| Realtime | WebSocket `/api/v1/events` (scan progress, session updates, request status) |
| Media tools | ffprobe (JSON) for analysis, ffmpeg subprocess per transcode/remux session |
| HW accel | Pluggable: VAAPI, QSV, NVENC, software fallback; detected at startup |
| Config | TOML file + env overrides; secrets in env |
| Deploy | Docker image (`/dev/dri` passthrough), Caddy in front for TLS |

### Server crates (`server/crates/`)
| Crate | Responsibility |
|---|---|
| `core` | Domain types, DB pool, migrations, config, error types |
| `scanner` | FS watch (notify), filename parsing, ffprobe ingest, incremental rescans |
| `metadata` | TMDB/TVDB clients, matching, `.nfo`/local art, image cache + resizing |
| `playback` | Device profiles, decision engine, transcode session manager, HLS packaging, trickplay |
| `iptv` | M3U/Xtream ingest, XMLTV EPG, shared upstream fan-out, GOP cache, warm channels |
| `requests` | Discovery, request workflow, Sonarr/Radarr clients, status tracking, notifications |
| `auth` | Users, sessions, invites, permissions, limits, 2FA/OIDC (later) |
| `api` | axum routes, OpenAPI, WebSocket, static web UI serving; `reel-server` binary |

## 3. Data model (initial)

```
users(id, username, password_hash, role[admin|user], created_at, disabled)
user_limits(user_id, max_streams, max_transcodes, max_bitrate_kbps, request_quota_movies, request_quota_tv, quota_window_days, auto_approve)
sessions_auth(id, user_id, token_hash, device_id, device_name, client, ip, last_seen, expires_at)
invites(id, code, created_by, library_ids, limits_json, expires_at, used_by, used_at)

libraries(id, name, kind[movies|shows], paths_json, scan_interval, created_at)
library_access(user_id, library_id)

media_items(id, library_id, kind[movie|show|season|episode], parent_id, title, sort_title, year,
            tmdb_id, tvdb_id, imdb_id, overview, rating, content_rating, runtime_s,
            season_no, episode_no, air_date, poster, backdrop, added_at, updated_at, locked_fields_json)
media_files(id, item_id, path, size, mtime, container, duration_s, bitrate, probe_json, edition, part_no)
streams(id, file_id, idx, kind[video|audio|subtitle], codec, profile, level, width, height,
        hdr[none|hdr10|hlg|dv], bit_depth, channels, language, title, default, forced, external_path)
people(id, tmdb_id, name, photo), credits(item_id, person_id, role, character, ord)
genres(id, name), item_genres(item_id, genre_id)
collections(id, name, tmdb_id), collection_items(collection_id, item_id)

watch_state(user_id, item_id, position_s, played, play_count, last_played_at)
watchlist(user_id, item_id | tmdb_id, added_at)

playback_sessions(id, user_id, device_id, file_id | channel_id, mode[direct|remux|transcode],
                  video_decision_json, started_at, last_ping, bitrate)

requests(id, user_id, media_type[movie|tv], tmdb_id, seasons_json, status, arr_kind, arr_id,
         quality_profile_id, root_folder, approved_by, created_at, updated_at, note)
issues(id, user_id, item_id, kind[video|audio|subtitle|other], message, status, created_at)
arr_servers(id, kind[sonarr|radarr], name, url, api_key, default_profile, default_root, is_4k)
download_clients(id, kind[qbittorrent|transmission], url, username, password)   -- optional, for speed/peers
request_downloads(request_id, arr_queue_id, download_id, state, release_title, size, last_progress, updated_at)
notification_targets(id, kind[discord|telegram|webhook|email], config_json, events_json)

iptv_sources(id, name, kind[m3u|xtream], url, username, password, max_connections, epg_url, refresh_interval)
channels(id, source_id, ext_id, name, number, group_name, logo, tvg_id, stream_url, catchup_json, enabled, favorite_rank)
channel_aliases(channel_id, alt_source_channel_id, priority)   -- failover
epg_programs(channel_tvg_id, start, stop, title, sub_title, description, category, icon, episode_num)
```

## 4. API surface (v1, prefix `/api/v1`)

- **Auth**: `POST /auth/login`, `POST /auth/logout`, `POST /auth/invite/{code}/accept`, `GET /me`
- **Libraries**: `GET/POST /libraries`, `POST /libraries/{id}/scan`, `GET /libraries/{id}/items?sort&filter&cursor`
- **Items**: `GET /items/{id}`, `GET /items/{id}/children`, `PATCH /items/{id}` (manual fix), `POST /items/{id}/match`
- **Home**: `GET /home` (continue watching, next up, recently added, per-library rows)
- **Search**: `GET /search?q=` (library + TMDB discovery merged, flagged by availability)
- **Images**: `GET /images/{kind}/{id}?w=` (resized, cached, ETag)
- **Playback**:
  - `POST /playback/{fileId}/decide` body = device profile → returns `{mode, url, audio/sub options, reasons}`
  - `GET /stream/{sessionId}/master.m3u8`, `/stream/{sessionId}/{variant}/{segment}.m4s`
  - `GET /direct/{fileId}` (range requests)
  - `POST /playback/{sessionId}/progress`, `DELETE /playback/{sessionId}`
  - `GET /items/{id}/trickplay/{w}/index.json`, subtitles `GET /subs/{streamId}.vtt`
- **Requests**: `GET /discover/{trending|popular|upcoming}`, `GET /discover/{movie|tv}/{tmdbId}`,
  `POST /requests`, `GET /requests?status`, `POST /requests/{id}/approve|decline`, `POST /issues`
- **IPTV**: `GET /iptv/channels?group`, `GET /iptv/epg?from&to&channels`, `POST /iptv/channels/{id}/tune` → session,
  `GET /iptv/live/{sessionId}/index.m3u8`
- **Admin**: users, invites, limits, arr servers, iptv sources, sessions (`DELETE` to kill), hw accel status, logs
- **Events**: `GET /events` (WebSocket)

## 5. Playback engine

1. Client sends a **device profile**: supported containers, video codecs (+profile/level/HDR/bit depth),
   audio codecs/channels, subtitle formats, max resolution/bitrate, and whether it uses native player or MSE.
2. Server decides per stream:
   - **Direct play**: container + all selected streams supported → serve file with range requests.
   - **Remux (direct stream)**: codecs OK, container not (e.g. MKV on Tizen/iOS) → ffmpeg `-c copy` to HLS fMP4.
   - **Audio-only transcode**: video OK, audio not (DTS/TrueHD → AAC/EAC3).
   - **Full transcode**: video unsupported or over bitrate cap → HW encode H.264/HEVC.
   - **Subtitles**: text → WebVTT sidecar; image (PGS/VobSub) on clients that can't render → burn-in (forces transcode).
3. Response includes human-readable `reasons[]` (shown in admin session dashboard).
4. **Transcode session manager**: one ffmpeg per session, segment-on-demand with seek = restart at nearest keyframe,
   idle kill after N seconds without segment requests, global + per-user concurrency limits, HW-first with SW fallback.
5. HDR → SDR tone-mapping (VAAPI/QSV `tonemap` / `zscale` fallback) only when the client lacks HDR.
6. Trickplay: background job produces sprite sheets + index per item.
7. Intro/credits detection (phase later): audio fingerprint comparison across episodes of a season.

TV-specific: LG/Samsung natively decode HEVC, HDR10, (some) DV, AC3/EAC3 → expect direct play/remux for most content.

## 6. IPTV engine (`crates/iptv`)

**Ingest**
- M3U (`#EXTINF` with `tvg-id`, `tvg-logo`, `group-title`, `catchup` attrs) and Xtream Codes (`player_api.php`).
- XMLTV EPG (gz supported), streamed parse (quick-xml) into `epg_programs`; window kept to −1d…+7d.
- Channel ↔ EPG mapping via `tvg-id`, then fuzzy name match; manual override in admin.
- Scheduled refresh; diffing to keep stable channel IDs and favorites.

**Live pipeline (the "optimised" part)**
```
provider ──1 conn──► Upstream task ──► ring buffer (bytes::Bytes, MPEG-TS packets)
                         │                     │
                         │             GOP cache (last keyframe → now)
                         ▼                     ▼
                  packager (ffmpeg -c copy → fMP4 HLS, 1s parts / LL-HLS)  ──► N viewers
                  (only transcodes audio if client can't decode, e.g. MP2/AC3 on browser)
```
- **One upstream connection per channel**, shared via `tokio::sync::broadcast`/ring buffer; refcounted, closed after
  a grace period once the last viewer leaves.
- **GOP cache**: new viewers start from the latest keyframe immediately → zap time ≈ network RTT + 1 segment.
- **Warm channels**: keep previous channel and top-N favorites pre-connected *if* provider connection budget allows.
- **Connection budget**: enforce `max_connections` per source; if exhausted, steal the least-recently-viewed warm
  (non-watched) channel; never exceed provider limit.
- **Health/failover**: detect stalls (no bytes for X s, PTS discontinuities); reconnect with backoff; switch to alias
  channel from another source.
- **Passthrough mode** for native players (Tizen AVPlay / webOS / ExoPlayer can play raw MPEG-TS) → zero packaging cost.
- Later: timeshift (disk-backed ring buffer), catch-up via Xtream `timeshift` URLs, DVR recordings into a library.

**UI**: virtualized EPG grid (channels × time), channel number entry & up/down zapping on TV remotes, mini-guide overlay.

## 7. Requests (built-in Seerr)

- Discovery via TMDB: trending, popular, upcoming, by genre, search; each card shows `available | partially | requested | none`
  computed by joining on `tmdb_id`/`tvdb_id` with the library.
- Request movie / full series / specific seasons. Per-user quota + auto-approve flags (`user_limits`).
- On approve → POST to Radarr (`/api/v3/movie`) or Sonarr (`/api/v3/series`) with profile/root/monitored seasons + search.
- Status tracking: poll Arr queue/history + Sonarr/Radarr webhooks (`/api/v1/webhooks/arr`) → `downloading` →
  when scanner ingests a matching item → `available` → notify requester.

### 7.1 One-click "Add" with live download progress (core UX)

**Flow**: user searches "Dune" → result card → **Add** → card immediately shows a progress ring that fills as the
torrent downloads → ring completes → card flips to **Play**.

- **Add** for admins / auto-approve users = create request + approve + push to Radarr with `addOptions.searchForMovie=true`
  in one call. Other users get `Requested` (pending approval) and see the same ring once approved.
- Defaults (quality profile, root folder, 4K instance) come from `arr_servers`; long-press / "⋯" opens advanced options.

**Status state machine** (per request, per episode for TV):
```
requested ─► (approved) ─► searching ─► queued ─► downloading(%) ─► importing ─► available
                              │            │            │
                              └► not_found  └► stalled ◄─┘   (any) ─► failed
```
| State | Source of truth |
|---|---|
| `searching` | Added to Radarr, no queue item yet (indexer search running) |
| `not_found` | Search finished (Radarr command completed) with no grab; auto-retry via Radarr's RSS monitoring |
| `queued` / `downloading` | Radarr `GET /api/v3/queue?includeMovie=true`: `progress = 1 - sizeleft/size`, `timeleft`, `status` |
| `stalled` | Queue `trackedDownloadState`/`status` warning, or no byte progress for N minutes |
| `importing` | Queue `trackedDownloadState = importPending/importing`, or webhook `Download` event |
| `available` | Scanner ingests a file whose `tmdb_id` matches (targeted rescan triggered by Radarr webhook) |
| `failed` | Webhook `DownloadFailed` / queue error — Radarr blocklists + re-searches automatically |

**Progress plumbing**
- `requests` crate runs a **download tracker** task: polls each Arr's `/queue` every 2s *only while* any request is in
  `searching|queued|downloading|importing`; idle otherwise. Webhooks (`Grab`, `Download`, `DownloadFailed`,
  `MovieFileDelete`) give instant transitions between polls.
- Optional **direct torrent-client adapter** (qBittorrent WebUI API / Transmission RPC), matched by `downloadId`
  (= torrent infohash) → finer data: speed, ETA, seeds/peers. Radarr queue alone is sufficient for the ring.
- Progress is **not persisted** per tick: kept in memory (`DashMap<request_id, Progress>`), state transitions persisted.
- Pushed to clients over WebSocket `/events` as
  `{type:"request_progress", request_id, tmdb_id, state, progress:0..1, eta_seconds?, speed_bps?}` (throttled to ≤1/s per item;
  wire format is snake_case throughout, auth via `?token=` query param on the WebSocket).
  Clients subscribe once; any card/poster with matching `tmdbId` renders the ring (search results, discover rows,
  "My requests" page, item detail).
- REST fallback: `GET /requests/active` returns current snapshot (on app load / reconnect).

**UI**: circular progress ring overlaid on the poster/Add button — indeterminate spinner for `searching`/`importing`,
determinate ring with % for `downloading`, amber for `stalled`, red for `failed`/`not_found` with retry action,
checkmark → **Play** on `available`. Detail page shows ETA, size, speed (if torrent adapter configured), release name.

TV series: same mechanics via Sonarr; ring shows aggregate season progress, detail page shows per-episode rings.
- Webhooks also trigger targeted rescans of the imported path (no full library scan).
- Notifications: Discord, Telegram, generic webhook (email later). Events: requested, approved, declined, available, issue.
- Issues: users flag broken audio/subs/video on an item; admin resolves.

## 8. Sharing & security

- Invite links (single-use, expiring) that preset library access + limits.
- Argon2id passwords; opaque session tokens (hashed at rest), per-device; revoke from admin.
- Limits enforced at `decide`/`tune` time: concurrent streams, concurrent transcodes, bitrate cap (forces lower variant).
- Rate limiting on auth routes; lockout/backoff; audit log for admin actions.
- Stream URLs are signed + short-lived (so they work in native players without auth headers).
- Remote exposure: domain + port forward + Caddy (auto TLS). Not Cloudflare Tunnel (ToS on video).
- Later: TOTP 2FA, OIDC (Authelia/Authentik).

## 9. Clients

| Target | Stack | Player |
|---|---|---|
| Web / PWA | SolidJS + Vite, shared `@reel/ui` | hls.js (MSE), native for direct-play MP4 |
| LG webOS | same UI, TV layout + spatial nav, packaged with `ares-cli` | `<video>` (native pipeline, HEVC/HDR) |
| Samsung Tizen | same UI, packaged `.wgt` with Tizen CLI | AVPlay API |
| Android TV + phone | Kotlin, Jetpack Compose (+ Compose for TV), Media3/ExoPlayer | ExoPlayer |
| iOS / tvOS (later) | Swift/SwiftUI | AVPlayer |

- `@reel/sdk`: generated from `openapi.json` (openapi-typescript + openapi-fetch).
- `@reel/ui`: components, routes, TV focus management (`@noriginmedia/norigin-spatial-navigation` or equivalent),
  `PlayerAdapter` interface with `HlsJsAdapter`, `WebOsAdapter`, `AvPlayAdapter`.
- Each app reports its device profile (static per platform + runtime capability probing).
- Target older TV browsers: build target ~Chromium 68 (webOS 5 / Tizen 5.5), avoid heavy CSS effects, virtualize lists.

## 10. Phases & acceptance criteria

| # | Phase | Done when |
|---|---|---|
| 0 | Foundation | Workspace builds; `reel-server` serves `/api/v1/health`; migrations run; OpenAPI exported; TS SDK generated; CI (fmt, clippy, test, typecheck) |
| 1 | Library + direct play | Add library → scan → TMDB-matched movies/shows browsable in web; direct-play MP4/H.264 in browser; incremental rescan on file change |
| 2 | Playback engine | Device-profile decisions; remux + HW transcode HLS; seek; text + burned subs; watch state, continue watching, next up |
| 3 | TV apps | webOS + Tizen apps installable on real TVs, remote nav, native player, direct-play HEVC/HDR |
| 4 | Sharing | Invites, library access, stream/transcode/bitrate limits, session dashboard with kill |
| 5 | Requests | Search → **Add** → Radarr grabs → live progress ring (searching → % → importing) → flips to Play; Sonarr equivalent; Discord/Telegram notifications |
| 6 | IPTV | M3U + Xtream + XMLTV; shared upstream; < 1s zap on LAN; connection budget; EPG grid on TV |
| 7 | Android | Android TV + phone app with ExoPlayer, IPTV zapping |
| 8 | Polish / later | Trickplay, intro skip, timeshift/catch-up, DVR, 2FA/OIDC, iOS |

## 11. Open decisions
- Server GPU: assumed Intel iGPU (VAAPI/QSV) — confirm; NVENC supported via same abstraction.
- IPTV source type(s): supporting both M3U and Xtream from the start.
- Final project name.
- Optional Jellyfin-compatible API shim to bridge iOS (Infuse/Swiftfin) before a native iOS app.

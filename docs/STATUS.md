# Status: 2026-11-12

Live build: **`65bd851`** (2026-11-11) on CT 113 → https://reel.rafikii.co (LAN + Tailscale only).

## What's verified on real hardware and data

- **Library:** 178 movies, 4 shows, 4 anime from NFS, with TMDB metadata. Rename detection, multi-episode files and targeted webhook rescans all work.
- **Playback:**
  - **Direct play:** MP4/H.264/AAC (5.1 included).
  - **Remux + audio transcode:** MKV with EAC3 audio. Keyframe-indexed and seek-safe; segment timelines were verified with tfdt checks.
  - **VAAPI transcode:** 1080p→720p at **8.3× realtime** on the N100.
- **Adaptive quality, server side:** a 6 Mbps user cap → 720p at ~4 Mbps. Per-user caps currently stand in for the missing client side.
- **Requests → Radarr/Sonarr → qBittorrent:** queue progress, qBittorrent client state, webhooks → available.
- **Picker, search, person pages, reviews (TMDB), Downloads page, player controls, subtitles overlay, automatic subtitle selection.**
- **IPTV:**
  - 11,950 channels / 175 groups; live HLS through the hub with VAAPI transcode.
  - Group browser; newest-wins on the single-connection plan.
  - **EPG ~1,436 channels / ~54k programmes** from external guides.
  - VOD catalog imported (41k movies / 15k series).
  - VOD **direct play** through the upstream proxy (#3, 2026-11-11): seeks resume in 0.4–0.6 s, exactly one
    provider connection, a stop frees it in ~2 s, and no provider login in ffmpeg args or logs. Remux seek is broken on this build (#92; fix built, not deployed).
- **Ops:** GitHub → GHCR → CT 113 deploys (one-click **deploy** workflow on the self-hosted runner CT 116; the `PROJECT_TOKEN` secret is needed for ticket updates); version shown in Settings → About; container resource limits; transcode caps (3 total, 2 per user).
- **Network:** Tailscale now **direct** (UDP 41641 static-port rule on OPNsense) → 9–15 Mbps remote, up from 3.5 Mbps over the DERP relay.

## Built but not exercised for real (🧪)

- **webOS/Tizen apps:** never installed on a TV. Packaging CLIs aren't installed; the Tizen package id is a placeholder.
- **Casting:** AirPlay and Chromecast never tested with devices. **DLNA** needs the TV IPs + a CT 113 firewall rule.
- **IPTV VOD remux/transcode playback** through the upstream proxy (#3; direct play is verified, remux seek is broken on `65bd851`, fix built: #92). Reviewed in code and
  covered by CI tests with local fake providers (2026-11-11):
  - Movie and series episode, direct play (proxied through `/vod/direct` with `Range`) and remux/transcode (ffmpeg
    reads a loopback proxy URL, `Range` passed through).
  - A remux seek never holds two provider connections, and stopping closes the connection. ffmpeg's own HTTP seek
    (5.1 in the image, 8.0 in CI) opens the new connection *before* closing the old one, so every seek (matroska
    cue lookup, restart at a new position) briefly needed two. The proxy now gives a session's input one
    connection: a new request cuts the previous one and waits for it to be released
    (`exclusive_input_closes_the_old_connection_before_a_seek_opens_the_next`,
    `remote_remux_seek_and_stop_keep_a_single_provider_connection`).
  - **A remux can be seeked** (#92, built and tested in CI, not deployed). On build `65bd851` (#3 check) a forced
    VOD remux played with one provider connection but could not be seeked: remote copy sessions skipped the
    keyframe index and served ffmpeg's EVENT playlist without ENDLIST, which hls.js treats as live
    (`duration = Infinity`, nothing seekable). The index is now read from the MKV Cues / MP4 `stss` with a few
    `Range` requests through the session's exclusive proxy token (cached per title), so the playlist is a full VOD
    playlist and a seek restarts ffmpeg at that keyframe, still with one provider connection
    (`remote_mkv_remux_is_seekable`, `remote_mp4_remux_with_moov_at_the_end_is_seekable`,
    `provider_without_range_support_falls_back_without_downloading_the_file`).
  - Takeover, stop or idle reap now also closes a **paused** direct-play stream (it used to keep the provider
    connection open until the browser let go).
  - The source's configured User-Agent is now used for every VOD request (probe, direct play, ffmpeg, subtitles);
    before, only the probe used it.
  - No provider URL or login in ffmpeg/ffprobe argv, client errors or the proxy's logs.
  - Live, read-only (2026-11-11, build `99b0deb`): CT 113 container logs (30 days) and `ps` had no provider login,
    Xtream credential paths or `username=`/`password=` values; the only provider URL in the logs is a masked
    guide URL (`username=***&password=***`); the running ffmpeg reads `pipe:0`. **Playback was not tried live**:
    someone was watching live TV, and the provider allows one connection.
- **Direct (local) Live TV playback** (#77): built and tested in CI, not yet tried on a TV or against the real provider. Off until `REEL_ALLOW_DIRECT_LIVE` is set on CT 113.
- **Live TV HLS upstreams through the upstream proxy** (#11): built and tested in CI with local HLS fixtures, not yet deployed. Only channels whose provider URL is HLS (`.m3u8` or an HLS content type) go through ffmpeg and the proxy; raw MPEG-TS channels are read by the hub directly, as before. Tokens of a playlist that is still in flight when its lease or session ends are no longer registered (they used to outlive the hub connection until their sliding expiry; `a_playlist_in_flight_when_its_owner_ends_registers_nothing`).
- **Notifications** (Discord/Telegram/webhook).
- **Player menus in full screen** (#94, not yet deployed). Checked 2026-11-15 with Playwright in mock mode: in real
  browser full screen (Chrome) the speed, audio, subtitle and "Play on…" sheets open over the video, work with
  mouse and keys, and close without leaving full screen; a toast shows too. Leaving full screen with a sheet open
  keeps it open with focus inside. 1920×1080 TV mode (arrow keys, Back) is unchanged. Not checked: Safari.
- **Player and connection resilience** (#91, #93, #81, not yet deployed). Checked 2026-11-12 with Playwright
  (mock mode, and the new web client against the live `65bd851` server; desktop and 1920×1080 TV mode):
  - Chrome had been playing HLS with its **built-in player**, not hls.js (`canPlayType(mpegurl)` is now "maybe"
    in Chrome), so the app had no say over buffering or retries; `CHUNK_DEMUXER_ERROR_APPEND_FAILED` in #91 came
    from it. hls.js is now used wherever MSE exists. Against the live server: start at 2:30 in ~10 s,
    `init.mp4` loaded once (the startup double-load is gone), and two connection resets on one segment were
    retried with no error.
  - Live TV on the old 6 × 1 s window (France 5): a `404 seg_00016` was recovered without an error; when the
    provider feed itself stalled (`upstream failed: stalled: no data for 8.0s`, five times in a minute) the player
    showed "Catching up with the live stream…" and then re-tuned once by itself. The longer server window (30 s
    + 20 s grace) is in CI only.
  - #81: with the browser offline, a page showed "Can't reach your Reel server" and filled in by itself ~1.4 s
    after the network came back; the events socket was replaced after a long absence from the tab.
  - Not checked: the 90 s VOD buffer riding out the 2:44–3:16 bitrate peak. The test Mac's link to the server
    was below the file's bitrate the whole time (10 downloads running), so it starved either way; that needs a
    link faster than the file's average. Chrome over HTTP/3 on Tailscale wasn't reproduced (Playwright went
    through the dev proxy over TCP).

## Known gaps / bugs

| Area | Gap |
|---|---|
| Player | **Adaptive quality client** (bandwidth estimate, auto step-down/up, quality menu) wasn't delivered (the agent errored). For now, set per-user `max_bitrate_kbps` (the admin is at 6000) |
| Player | A link slower than a file's peak bitrate still stalls (e.g. The Remarried Empress E1: 5.9 Mbit/s average, 11–13 Mbit/s at 2:44–3:16). The player now waits and recovers instead of stopping (#91), but only a lower rendition fixes it (#87) |
| Network | Chrome over HTTP/3 on Tailscale (MTU 1280) sees `QUIC_TOO_MANY_RTOS` and status-0 responses (#91, #93). The player and the app now retry these; turning off HTTP/3 for reel.rafikii.co at Caddy (CT 107) is an owner decision, not changed |
| IPTV | Some provider feeds stall for 8 s at a time (France 5 on 2026-11-12); the hub reconnects and the player re-tunes, but the picture freezes meanwhile |
| Reviews | IMDb/RT/Metacritic scores need `REEL_OMDB_API_KEY` |
| EPG | Al Jazeera guide fully covered by AE1 (fine). The EG1 guide is stale upstream (removed) |
| IPTV | Live HLS has no quality ladder (live transcode is fixed) |
| IPTV | A VOD remux of a file without a container keyframe index (MPEG-TS, AVI), or from a provider that ignores `Range`, still serves ffmpeg's EVENT playlist and can't be seeked (#92). Transcodes and MKV/MP4 remuxes are seekable |
| Library | Episodes of a multi-episode file all start at 0:00 (no per-episode offsets) |
| Downloads | "Common Side Effects" is stuck in Sonarr ("episodes not imported"). That's a Sonarr-side import problem, not Reel |
| Requests | **Torrent searches fail in bursts, outside Reel** (#66). Root cause: gluetun's DNS-over-TLS to Cloudflare (1.1.1.1:853) times out or gets reset. Prowlarr runs inside gluetun, so every indexer fails at the same moment (`EAI_AGAIN`: 119 of 165 indexer errors from 2026-10-30 to 11-10). The same DoT path feeds gluetun's health check, which caused VPN reconnect loops of 4 h (11-03) and 3 h (11-05). Prowlarr's backoff then reaches Radarr/Sonarr as HTTP 429, and they escalate their own backoff up to 12 h. Requests made in that window search with **0 active indexers**, and Reel marks them `not_found`. There are no Cloudflare/VPN-IP blocks, and Prowlarr↔Arr sync is fine. Fix awaiting owner approval (CT 105): switch gluetun to Proton's in-tunnel DNS, then Retry the 8 `not_found` requests (releases exist for them). Surfacing this in Reel is covered by #67 |
| Toolchain | rustc pinned to 1.86 (some crates pinned for it, e.g. wiremock 0.6.4). sqlx 0.9 needs 1.94, so sqlx stays on 0.8 and its bundled SQLite advisory (RUSTSEC-2026-0459, #80) is open |

## Security posture

A three-reviewer council ran on 2026-11-03 (a general reviewer, plus Sonnet and GPT council members). It found
**no Critical issues** and established no auth bypass, SQL injection, path traversal or shell injection.

**Fixed and deployed:**

- Login: Argon2 concurrency slots, failures recorded before verification, (user, IP) lockout, /64 buckets, input caps, sampled audit, bounded limiter.
- First-run `/setup` restricted to the LAN or a one-time token; `trusted_proxies` client-IP trust; signed URLs with `sign_epoch`; 90-day absolute sessions.
- Per-user permissions (IPTV / VOD / requests / casting); DLNA admin-only by default; Arr routing options admin-only.
- Webhooks: secret of 32+ characters, header/Basic auth, constant-time comparison, per-IP lockout, `..` paths rejected.
- WebSocket tickets; sockets closed on revoke; logs no longer contain query strings; safe post-login redirect.
- Playback errors are generic (stderr redacted server-side); atomic stream/transcode limits; global ffmpeg process budget; single-flight subtitle conversion; no software HEVC.
- **Upstream proxy for ffmpeg/ffprobe** (SSRF policy + credentials out of argv) for VOD, remote subtitles and remote probes.
- Local ffprobe/ffmpeg restricted to `-protocol_whitelist file`; symlink-safe scanning; two-level subtitle filter escaping; scratch-disk budget.
- `stream_guard` decodes the session id (no %-encoded owner-check bypass); `/health` build info and `openapi.json` only for signed-in users.
- Container: memory 3 GB, PIDs 512, CPUs 3; transcode caps.

**Still open (must be done before any public exposure):**

1. ~~Upgrade `quick-xml` to ≥ 0.41; sqlx with sqlite only~~ Fixed (#7, ✅ live): `quick-xml` 0.42 (RUSTSEC-2026-0194/-0195 gone) in the XMLTV and DLNA parsers. sqlx was already built SQLite-only; every crate now uses the one workspace entry and CI fails if `sqlx-mysql`, `sqlx-postgres` or `rsa` enter the build graph. `rsa` (RUSTSEC-2023-0071) stays listed in `Cargo.lock` only, because cargo records disabled optional deps, and is ignored with that reason in `server/.cargo/audit.toml`. Verified on `65bd851` (2026-11-11): guide refresh OK, `&` and accents correct, migrations clean. DLNA discovery on the real TV still to check. A fresh `cargo audit` also found RUSTSEC-2026-0459 (bundled SQLite in `libsqlite3-sys` 0.30), tracked in #80: the fix needs sqlx 0.9, which needs rustc 1.94.
2. ~~IPTV/EPG ingestion budgets: bytes, gzip expansion, line/token sizes, record counts, deadlines~~ Fixed (#8, ✅ live): playlist, guide and Xtream API downloads have byte caps and deadlines, gunzipped guides an expanded-size cap, M3U lines and XML events/attributes/texts length caps, channel/programme/VOD/category lists count caps, and downloaded guides are deleted on every exit path. Limits in `server/crates/iptv/src/limits.rs` (ARCHITECTURE A21). Verified on `65bd851` (2026-11-11): a full refresh succeeds (11,950 channels, 41,338 movies, 15,147 series, ~54k programmes), no limit errors, no leftover temp files.
3. Logo/TMDB image fetches: streaming size cutoff, single-flight, cache size budget.
4. ~~Local artwork: canonicalize under the library roots + magic-byte sniff~~ Fixed in code (#10, 🧪): the image route resolves symlinks, requires a library root, caps size and sniffs magic bytes. Counts as done once deployed and checked on the real library.
5. ~~SSRF guard gaps (IPv4-compatible v6, `64:ff9b::/96`, `2002::/16`, `192.0.0.0/24`); route **live IPTV** HLS upstreams through the upstream proxy~~ Fixed (#11, ✅ live): `net.rs` judges NAT64, 6to4, Teredo and IPv4-translated addresses by the IPv4 address they carry, refuses `::a.b.c.d`, `64:ff9b:1::/48`, `192.0.0.0/24`, `100::/64` and `fec0::/10`, and the guarded client no longer sends the previous (login) URL as `Referer` on redirects. The live hub hands ffmpeg only a loopback proxy URL. Verified on `65bd851` (2026-11-11): no provider login in ffmpeg args (472 samples) or logs. The provider has no HLS channels, so the live-HLS proxy path is covered by CI only. Live TV stalls are tracked in #93.
6. DLNA discovery trust: bind control URLs to the responding IP, cap body sizes, setting to disable discovery.
7. Live IPTV transcodes counted against the user's `max_transcodes`.
8. Signing key / data dir permissions created atomically (0600/0700).
9. Redact `epg_urls`/source URL credentials in admin DTOs; filter `library_item_id` in Discover/Downloads by library visibility.
10. Edge, if ever public: only Caddy 443 forwarded; block `/api/v1/{admin,setup,webhooks,cast}/*` from the WAN; CSP/Permissions-Policy at Caddy; rate limits/CrowdSec; geo allowlist.

**Decision on record:** Reel stays **private** (LAN + Tailscale) until items 1–10 are done and re-reviewed.

## Backlog (next)

1. Adaptive quality in the player (bandwidth probe, auto step, quality menu), then remove the admin's 6 Mbps cap.
2. Close the security items above.
3. Verify webOS/Tizen on real TVs; DLNA with the TV IPs; AirPlay/Chromecast.
4. IPTV VOD playback check on the provider (#3, owner); live HLS quality ladder.
5. OMDb key for IMDb/RT scores.
6. Trickplay thumbnails (the scrubber already has the slot), intro/credits skip.
7. Android TV / phone app (needs the Android SDK), iOS later.
8. Per-episode offsets for multi-episode files.

# Status: 2026-11-15

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
- **Live TV transcodes under the transcode limits** (#13): built and tested in CI
  (`live_transcodes_count_against_the_transcode_limits` with an MPEG-2 fake provider,
  `external_transcodes_count_against_the_limits`), not yet deployed.
- **Provider login hidden in admin responses; library links scoped to visible libraries** (#15): built and tested in CI
  (`admin_source_responses_never_contain_provider_credentials`,
  `discover_cards_link_only_library_items_the_viewer_can_open`, the downloads test), not yet deployed.
- **Notifications** (Discord/Telegram/webhook).
- **Player menus in full screen** (#94, not yet deployed). Checked 2026-11-15 with Playwright in mock mode: in real
  browser full screen (Chrome) the speed, audio, subtitle and "Play on…" sheets open over the video, work with
  mouse and keys, and close without leaving full screen; a toast shows too. Leaving full screen with a sheet open
  keeps it open with focus inside. 1920×1080 TV mode (arrow keys, Back) is unchanged. Not checked: Safari.
- **Settings → Playback inside the settings layout** (#88, not yet deployed). Checked 2026-11-15 with Playwright in mock
  mode: desktop (nav kept, Playback highlighted, back/forward, prefs save in the pane), as a non-admin (rail → Settings
  opens Playback; nav shows Playback + About; `/admin/…` says admin-only), the player sheet's "Subtitle & audio
  preferences" link, and 1920×1080 TV mode (Right from the Playback link enters the pane, Left returns to it, up/down
  stay on their side). Not checked live yet.
- **Pause and session loss** (#95, #100, #96; deployed in `bc6c69b`, owner verification pending). A paused player keeps its
  converted session: it pings `paused: true` every 10 s (up to 30 min of pause), and the server gives such a session
  30 min instead of 60 s (`paused_session_*`/`an_abandoned_paused_session_*` tests). A session the server ended
  anyway is reopened at the same position, right away if playing and on the next play if paused. A recovery (reload,
  decoder restart) or an automatic reopen no longer starts a paused video: that was #100 (the adapter called
  `play()` after every recovery step, and the app's reopen used `autoplay: true`). Checked 2026-11-15 with Playwright
  in mock mode, desktop and 1920×1080 TV mode (Enter on the play button): paused + session reaped → stays paused, no
  overlay → play reopens at the same spot; a media error while paused → reloaded at the playhead, still paused (the
  old code started playing by itself); reaped while playing → reopened within ~10 s, still playing.
  #96 (The Mongoose stalls): reproduced live on `70d03eb`. Chrome's HTTP/3 connection over Tailscale wedged in 3 of 5
  runs (segment headers arrive, the body never does, and every request on the connection hangs, progress pings
  included), so the server saw nothing and reaped the session. 2 of 2 runs with `--disable-quic` played past the first
  minute. The transport is #108 (owner decision at Caddy); the player now reopens the session once the connection
  is back, and a stall's error says whether a segment is stuck in flight.
  Live on `bc6c69b` (2026-11-15, Playwright, Chrome over Tailscale, The Mongoose from 85 s; desktop and 1920×1080
  TV mode with arrow keys and Enter): over TCP (`--disable-quic`), a 5.5 min pause sent 33 `paused: true` pings
  10 s apart, the session stayed on the server the whole time (no reap), the video stayed on the same frame and
  resumed from it on play; with its session ended while paused it stayed paused without an error, and play opened
  a new session at the same spot. Over HTTP/3 the same start wedged again (`segment 9 requested … ago and still
  loading` while the segment was ready on the server), and the automatic reopen's `decide` hung on the same
  connection ("Can't reach your Reel server"): that's #108, not fixable in the app. The stall message after a
  rebuild reported the old pipeline's last segment; it now reports the rebuilt player's own state (playlist in
  flight, or nothing since the restart).
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

## Playback baseline (2026-11-15, build `70d03eb`, #102)

Measured read-only, with the admin's 6 Mbit/s cap in place. Full numbers, the comparison with YouTube and the
proposal are in [#102](https://github.com/mrafik92/reel/issues/102#issuecomment-6887831224).

- **Test paths.** Desktop Chrome on a Mac **outside the home LAN, over Tailscale** (direct, RTT 100–230 ms, raw
  throughput swinging 4–27 Mbit/s, default HTTP/3). Server-side numbers are from loopback inside CT 113, so they
  show the pipeline without the network. A browser on the home LAN wasn't measured.
- **Browser over Tailscale:**

  | Title | Mode | Delivered | Decide → first frame | Seek +45 s / to 50 % |
  |---|---|---|---|---|
  | Paranormal Activity (1080p H.264 MP4, 2.7 Mbit/s) | direct play | 1080p, original | 8.9 s (the `moov` is at the end: 4.1 MB tail fetch first) | 9.5 s / 4.1 s |
  | Meet Joe Black (1080p HEVC 10-bit MKV, 6.5 Mbit/s) | transcode 720p (cap) | 720p H.264 ~4 Mbit/s | 1.8 s | 12.7 s / 4.0 s |
  | Spider-Man 2002 (1080p H.264 + EAC3, 24.7 Mbit/s) | transcode 720p (EAC3 + cap) | 720p H.264 ~4 Mbit/s | 3.0 s | 4.2 s / 11.8 s |
  | Frankenstein (4K HEVC DV/HDR10, 25 Mbit/s) | transcode 720p, tone-mapped to SDR | 720p H.264 SDR | 9.9 s | 6.8 s / 9.7 s |

  Opening the player page adds 2.3–4.1 s before decide. Every stall (2–4 per ~70 s run, 0.3–5.3 s) followed a
  seek, on a fixed 4–5.5 Mbit/s rendition while the link dipped; one never recovered (#108).
- **Server pipeline (no network):** decide 0.04–0.06 s; first segment ready 0.7–0.8 s after decide for 1080p
  sources and 2.6 s for the 4K DV tone-map; far seek 1.0–2.7 s; transcodes run at 8.3× / 5.9× / 2.6× realtime.
  CT 113 serves a file at ~1.9 Gbit/s on the host bridge, so the server isn't the throughput bottleneck.
- **N100 (VAAPI, ffmpeg 5.1, iHD 23.1.1):** remux 107×; 1080p→720p H.264 9×; 1080p HEVC encode 3× (6× with
  `-low_power 1`); 3-rung H.264 ladder 2.5× (4.3× low-power); 4K DV tone-map 2.6×; two concurrent 1080p→720p
  4.5× each. H.264/HEVC (Main10)/VP9 encode in hardware; **no AV1 encode**. Plex shares the GPU on CT 113.
- **Effect of the cap (estimate from probe data, Chrome on this Mac):** 124 of 311 files (40 %) get a video
  transcode, 95 of them only because of the cap; without it, 29 (9 %: HDR/4K/10-bit H.264).

## Known gaps / bugs

| Area | Gap |
|---|---|
| Player | **Adaptive quality client** (bandwidth estimate, auto step-down/up, quality menu) wasn't delivered (the agent errored). For now, set per-user `max_bitrate_kbps` (the admin is at 6000) |
| Player | A link slower than a file's peak bitrate still stalls (e.g. The Remarried Empress E1: 5.9 Mbit/s average, 11–13 Mbit/s at 2:44–3:16). The player now waits and recovers instead of stopping (#91), but only a lower rendition fixes it (#87) |
| Network | Chrome over HTTP/3 on Tailscale (MTU 1280) sees `QUIC_TOO_MANY_RTOS` and status-0 responses (#91, #93), and the QUIC connection can wedge: response bodies stop mid-transfer and every request on it hangs (3 of 5 test runs on The Mongoose, 0 of 2 with QUIC off; #96, #108). The player and the app retry, and the player reopens a reaped session once the connection is back, but the picture freezes meanwhile. Turning off HTTP/3 for reel.rafikii.co at Caddy (CT 107) is an owner decision, not changed (#108) |
| Player | **Adaptive quality client** (bandwidth estimate, auto step-down/up, quality menu) isn't delivered. PR #69 (re-decide per switch) is open but not recommended as is (#102). For now, set per-user `max_bitrate_kbps` (the admin is at 6000) |
| Player | **One rendition per session** (A6): no multi-variant playlist, so the player can't step down on a link dip or start low and climb (#111). A link slower than a file's peak bitrate still stalls (e.g. The Remarried Empress E1: 5.9 Mbit/s average, 11–13 Mbit/s at 2:44–3:16). The player now waits and recovers instead of stopping (#91), but only a lower rendition fixes it (#87, #111) |
| Player | The 6 Mbit/s admin cap sends 95 of 311 files to a 720p transcode that the LAN doesn't need (#2, after #87) |
| Player | HDR/DV files are always tone-mapped to SDR in the browser (the profile probe reports no HDR), and the tone-map is the slowest path (2.6× realtime, 9.9 s to first frame remotely) (#114) |
| Player | Start-up costs on slow links: VOD playlists are 175–260 KB and served uncompressed (7.8 KB gzipped); MP4s with a trailing `moov` make the browser fetch the index from the end of the file first; transcode segments are 4 s from the start (#112) |
| Transcode | VAAPI encodes don't use the N100's low-power encoders, and output is always H.264 even for HEVC-capable clients (#113) |
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
3. ~~Logo/TMDB image fetches: streaming size cutoff, single-flight, cache size budget~~ Fixed in code (#9, 🧪): bodies are cut off at a byte cap (TMDB 32 MiB, logos/VOD 2 MiB) or deadline (30 s / 10 s) while streaming, concurrent fetches of one image share a single download (the lock map is emptied as fetches finish), failures are remembered briefly, and the TMDB and IPTV image caches have disk budgets with LRU eviction (`REEL_IMAGE_CACHE_MAX_MB`, `REEL_IPTV_IMAGE_CACHE_MAX_MB`; ARCHITECTURE A33). Counts as done once deployed and checked live.
4. ~~Local artwork: canonicalize under the library roots + magic-byte sniff~~ Fixed in code (#10, 🧪): the image route resolves symlinks, requires a library root, caps size and sniffs magic bytes. Counts as done once deployed and checked on the real library.
5. ~~SSRF guard gaps (IPv4-compatible v6, `64:ff9b::/96`, `2002::/16`, `192.0.0.0/24`); route **live IPTV** HLS upstreams through the upstream proxy~~ Fixed (#11, ✅ live): `net.rs` judges NAT64, 6to4, Teredo and IPv4-translated addresses by the IPv4 address they carry, refuses `::a.b.c.d`, `64:ff9b:1::/48`, `192.0.0.0/24`, `100::/64` and `fec0::/10`, and the guarded client no longer sends the previous (login) URL as `Referer` on redirects. The live hub hands ffmpeg only a loopback proxy URL. Verified on `65bd851` (2026-11-11): no provider login in ffmpeg args (472 samples) or logs. The provider has no HLS channels, so the live-HLS proxy path is covered by CI only. Live TV stalls are tracked in #93.
6. ~~DLNA discovery trust: bind control URLs to the responding IP, cap body sizes, setting to disable discovery~~ Fixed in code (#12, 🧪): an SSDP `LOCATION` must be on the IP that answered, AVTransport/`URLBase` URLs must stay on the description's host (a foreign volume or icon URL is dropped), descriptions (256 KiB) and SOAP answers (64 KiB) are capped, redirects are not followed, an SSDP answer can't take over a TV added by IP, and `REEL_CAST_DISCOVERY=false` turns SSDP off (ARCHITECTURE A34). Counts as done once deployed and checked live.
7. Live IPTV transcodes counted against the user's `max_transcodes`.
6. DLNA discovery trust: bind control URLs to the responding IP, cap body sizes, setting to disable discovery.
7. ~~Live IPTV transcodes counted against the user's `max_transcodes`~~ Fixed in code (#13, 🧪): a live HLS session whose shared packager re-encodes video counts as one transcode of its viewer (joining a running packager included) and each such packager as one server-wide, together with playback transcodes, under one admission lock. A user's own oldest live transcode gives way when they zap (newest wins); playback transcodes and other users' never do. Passthrough, direct and copy/audio-only HLS don't count. Counts as done once deployed and checked live.
8. Signing key / data dir permissions created atomically (0600/0700).
9. ~~Redact `epg_urls`/source URL credentials in admin DTOs; filter `library_item_id` in Discover/Downloads by library visibility~~ Fixed in code (#15, 🧪): admin source responses mask the source URL, guide URLs and the Xtream username; edits accept the masked values back, refuse a URL that still holds `***`, and need the login again to move an Xtream source to another server. Discover (lists, search, detail), person pages, global search and Downloads link only a copy of a title in a library the viewer can open. Counts as done once deployed and checked live.
10. Edge, if ever public: only Caddy 443 forwarded; block `/api/v1/{admin,setup,webhooks,cast}/*` from the WAN; CSP/Permissions-Policy at Caddy; rate limits/CrowdSec; geo allowlist.

**Decision on record:** Reel stays **private** (LAN + Tailscale) until items 1–10 are done and re-reviewed.

## Backlog (next)

1. Playback like YouTube (#102 proposal, awaiting owner approval): quality picker with a per-device default (#87),
   then remove the admin's 6 Mbps cap (#2); multi-variant HLS with a lazy low rung and hls.js ABR (#111, replaces
   PR #69's re-decide stepping, #1); faster first frame (#112); low-power/HEVC encodes (#113); HDR in the browser
   (#114); optional background pre-encode (#115).
2. Close the security items above.
3. Verify webOS/Tizen on real TVs; DLNA with the TV IPs; AirPlay/Chromecast.
4. IPTV VOD playback check on the provider (#3, owner); live HLS quality ladder.
5. OMDb key for IMDb/RT scores.
6. Trickplay thumbnails (the scrubber already has the slot), intro/credits skip.
7. Android TV / phone app (needs the Android SDK), iOS later.
8. Per-episode offsets for multi-episode files.

# Status: 2026-11-18

Android startup diagnostics (#210) are built for debug measurement. A synthetic MP4 resume run completed 20 warm attempts per client on local loopback, with Android emulator p50/p95 543/598 ms and desktop Chrome 131/148 ms, zero recorded failures. These different environments are a harness check. Cold, LAN, Tailscale, physical phone, real remux/transcode and controlled IPTV VOD remain unmeasured.

Live build: **`1a39b38`** (2026-11-15) on CT 113 → https://reel.rafikii.co (LAN + Tailscale only).

Android shared visuals and TV focus (#209), and portrait phone Home (#212), are built and checked on emulators with mock data. Physical-device verification remains pending.

Android detail hierarchy (#213) is built for library movies, shows, seasons and episodes and IPTV movies and series. Phone and 1920×1080 TV emulator checks use a local mock for artwork, playback targets, season switching and Back. Physical-device verification remains pending.

Reel Web for Android (epic #225), a WebView client next to the Compose app, has its shell built (#226) and Back, full screen and insets (#227): checked on an Android 15 phone emulator against the live server (setup, sign-in kept across a force stop, Search → title → Back, external links, TLS refusal, error screens) and a dev build of the web app (full screen, Back order, rotation, keyboard). The web half of #227 (Search scroll and focus on return, Back on the sign-in page) reaches the live server with the next deploy. The Android TV check (#231) passed on the Android TV emulator with D-pad and remote keys only, so the APK now has a Leanback launcher entry and banner. Its web fixes (Back on Home, play/pause keys, the player's control row) and TV mode (#229) need that deploy too. Not on a real phone or TV yet, no release, and playback in WebView is only smoke-tested (#230).

## What's verified on real hardware and data

- **Library:** 178 movies, 4 shows, 4 anime from NFS, with TMDB metadata. Rename detection, multi-episode files and targeted webhook rescans all work.
- **Playback:**
  - **Direct play:** MP4/H.264/AAC (5.1 included).
  - **Remux + audio transcode:** MKV with EAC3 audio. Keyframe-indexed and seek-safe; segment timelines were verified with tfdt checks.
  - **VAAPI transcode:** 1080p→720p at **8.3× realtime** on the N100.
- **Adaptive quality, server side:** a 6 Mbps user cap → 720p at ~4 Mbps. The admin's cap has since been removed (#2).
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

- **Late subtitles (#237):** follow-up import scans and six-hour reconciliation are built; pending deploy and verification. A manual Movies scan on 2026-11-18 registered Frida’s subtitle on the live server.

- **Crimson + cream retheme** (#177): built and checked in mock mode (desktop + 1920×1080 TV); not yet deployed or checked live.
- **YouTube-style click** (#183): a mouse click plays/pauses (double-click = full screen, undoing it); a touch tap shows the controls and a double-tap on a side seeks ±10 s, adding up. Checked with Playwright in mock mode (mouse 1280×800, touch 390×844, 1920×1080 TV with arrow keys); not yet deployed or checked on a real phone.
- **Phone bottom bar** (#180): five tabs + More (sheet with the rest); the inverted rail focus is keyboard/TV only, so a tapped tab no longer stays dark. Checked with Playwright (touch emulation at 360/390/430 px, desktop, 1920×1080 TV with arrow keys) in mock mode and against the live API from a dev build; not yet deployed or checked on a real phone. `viewport-fit=cover` is new: iPhone landscape (player, Live TV) needs a look on the device.
- **Reel Web for Android** (#226, #227): the WebView shell, Back order and full-screen video, checked on a phone emulator only (live server, and mock data from a dev build for #227). Known gap for #228: an unroutable address shows a blank page until Chromium's own connect timeout. A reported black bottom half after hiding the keyboard didn't reproduce on the emulator.
- **webOS/Tizen apps:** the webOS package launches and connects on the LG TV. The elapsed-time fallback (#131) and Live TV fullscreen sizing fix (#132) are built and awaiting hardware verification in the LG browser and app. Tizen has yet to be installed on a TV, and its package ID is a placeholder.
- **Casting:** AirPlay and Chromecast never tested with devices. **DLNA** needs the TV IPs + a CT 113 firewall rule.
- **Reel Web on Android TV** (#231): checked on the Android TV emulator only (D-pad and `adb` key events, a dev build of the web app against the live API), not on a real TV or remote. Live TV and the IPTV source switch leave focus on the rail/body for one press (#243). An HEVC + E-AC3 title stalled on the emulator (audio transcode, no segment in 25 s), which is #230's to classify.
- **Web identity in Reel Web for Android** (#229): client `android-web`, "Reel Web on <model>", TV mode from the ` (TV)` token, Chromecast off in the shell. Checked with Playwright using spoofed shell user agents (mock mode, and the live API storing the session); not deployed, not yet seen through the real APK on an emulator or phone.
- **IPTV movie genres** (#239): the live catalog had 22 tagged movies out of 41,368. The bulk import, TMDB backfill and full-catalog picker are built. Live backfill coverage is pending deployment.
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
- **Direct (local) Live TV playback** (#77): built and tested in CI, not yet tried on a TV or against the real provider. `REEL_ALLOW_DIRECT_LIVE=true` **is set on CT 113** (found by #16, 2026-11-15), so every user with IPTV access is offered it and can receive the provider URL with its login; this must change before public exposure (#126).
- **Live TV HLS upstreams through the upstream proxy** (#11): built and tested in CI with local HLS fixtures, not yet deployed. Only channels whose provider URL is HLS (`.m3u8` or an HLS content type) go through ffmpeg and the proxy; raw MPEG-TS channels are read by the hub directly, as before. Tokens of a playlist that is still in flight when its lease or session ends are no longer registered (they used to outlive the hub connection until their sliding expiry; `a_playlist_in_flight_when_its_owner_ends_registers_nothing`).
- **Compressed HLS playlists** (#112, item 1): built and tested in CI, not yet deployed. Every `.m3u8` response is sent brotli/gzip when asked (`playlists_are_compressed_media_is_not`: master and media playlists decode to the same bytes; segments and direct play come back uncompressed, and a ranged direct-play request is still a `206` with the exact slice). Local run, 2.5 h title (2,250 segments), signed media playlist: 216,234 B plain → 6,512 B gzip → 2,788 B br; hls.js 1.5 in Chrome parsed the br playlist and buffered the first segment. Still open in #112: short first segments, trailing-`moov` MP4s, hls.js startup tuning.
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
- **Playback quality choice** (#87, not yet deployed). Checked 2026-11-16 with Playwright, mock mode and the dev build
  against the live server (nobody else watching, progress pings stubbed): Settings → Playback → Quality saves per
  device and every decide sends it; the player's Quality sheet re-decides from the playhead and the server replaces
  the session. Live: Original on Spider-Man (H.264 + EAC3 MKV) → video copy, audio-only transcode; 720p on the same
  file → VAAPI transcode with "quality 720p requested"; Original on Frankenstein (4K DV) → the "Can't play the
  original here" card, then Play transcoded → tone-mapped 1080p. User-limit wording checked in mock only (no user
  has a limit live). 1920×1080 TV mode with arrow keys (mock and live): the button, the sheet and the card's
  Play transcoded are all reachable, and focus stays on Quality after a switch. Over this Mac's remote link
  (~2 Mbit/s that day) Original at 25 Mbit/s can't keep up and stalls; 720p plays.
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

- **7.1 AAC in the browser** (built, 🧪, not deployed): a file whose main audio is 7.1 AAC (Guardians of the Galaxy
  Vol. 3, file 56) stopped at 0:00 with "mediaSourceRequiresReset" in Chrome, on the owner's Android phone and on
  desktop. The server copied the 8-channel AAC into the remux, and Chrome's MSE AAC parser refuses 7.1 AAC (this file
  signals it with a program config element; ffmpeg's own 7.1 AAC uses channel config 12, also refused). MSE players now
  get 7.1 AAC as 5.1 AAC with the video copied; 5.1 AAC and 7.1 Opus are still copied (checked in Chrome with the
  file's real audio track). Not caused by #111 ①: the same title fails the same way on the build before it.

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

- **Honest connection errors** (#90, not yet deployed). Checked 2026-11-16 with Playwright on a production
  build (`vite preview`), with failures simulated by route interception: a dropped connection on Live TV shows
  "Couldn't load your channels / Check your connection and try again" with **Try again** and no `?mock=1` tip
  (the tip is compiled out of production bundles); a 503 shows the server's message; browser offline shows
  "You're offline"; a failed refresh of an already-loaded Live TV list keeps the list and shows a toast; in
  1920×1080 TV mode an arrow key lands on **Try again** and it stays focused through the background retries
  (the card is no longer swapped for the skeleton on each attempt). Mock mode unchanged. Signed in against the
  live server (`8b5fbbb`-era server, channel list only, nothing tuned): the list loads; with `iptv/*` requests
  reset, a fresh load shows the card and Try again recovers, and a refresh of an already-loaded list keeps it with
  a toast. Cancelled requests are covered by unit tests (`clients/apps/web/test/http.test.ts`). Not checked: a
  real outage with the container stopped.

## Known gaps / bugs

| Area | Gap |
|---|---|
| Network | Chrome over HTTP/3 on Tailscale (MTU 1280) saw `QUIC_TOO_MANY_RTOS`, status-0 responses and wedged QUIC connections (#91, #93, #96, #108). **Applied 2026-11-15 on CT 107:** Reel's LAN handler sends `Alt-Svc: clear` and h3 is off on the public `:8443` listener, so Chrome uses HTTP/2 for reel.rafikii.co (checked: a reload over Tailscale was all h2). Other LAN sites keep h3. Not yet re-checked: a long playback over Tailscale without wedges (#108) |
| Ops | Caddy's packaged unit ran `caddy run --environ`, which wrote CT 107's Cloudflare token (`CF_API_TOKEN`) to the journal at every start. `--environ` is removed (drop-in, 2026-11-15) and later starts are clean, but the token is **not rotated yet** (owner deferred it) and the old journal entries are still there; purge them after the rotation (OPERATIONS → Public edge) |
| Player | **Automatic quality is built, not verified** (🧪, #111 ③, #1): hls.js browsers get a multi-variant session, a stored bandwidth estimate and local quality switches (see FEATURES). Checked against the live server on 2026-11-18 from a slow remote link (~0.5–2 Mbit/s): Meet Joe Black started on its 480p HEVC rung, stayed there with no stall, and pinning Original started the original variant lazily without a new decide. Not yet measured: the design's 3.5 Mbit/s throttled run (no stall > 1 s over 5 min, original within 30 s of unthrottling) and the LAN first-frame/far-seek times. Safari, webOS/Tizen, casts and IPTV VOD still have no automatic quality: on a slow link, pick 720p/480p (each pick reopens the session) |
| Player | **One rendition per session**: no multi-variant playlist, so the player can't step down on a link dip or start low and climb. #111 replaces this (A6, design approved: [design/111-multivariant-hls.md](design/111-multivariant-hls.md)). Part ① (aligned variants, #182) is merged. Part ② is built and tested (🧪): the variant set, the multi-variant master, lazy rung admission with the transcode caps re-checked, idle stop (30 s), and the API fields (`adaptive`, `bandwidth_kbps`, `variants`, `start_variant`, Now playing's `variants`/`delivering`). Part ③ (the web client) is built and tested (🧪) and in review. A link slower than a file's peak bitrate still stalls (e.g. The Remarried Empress E1: 5.9 Mbit/s average, 11–13 Mbit/s at 2:44–3:16). The player now waits and recovers instead of stopping (#91), but only a lower rendition fixes it (#87, #111; with ③ the hls.js player steps down by itself) |
| Player | HDR/DV files are always tone-mapped to SDR in the browser (the profile probe reports no HDR), and the tone-map is the slowest path (2.6× realtime, 9.9 s to first frame remotely) (#114) |
| Player | Start-up costs on slow links: MP4s with a trailing `moov` make the browser fetch the index from the end of the file first; transcode segments are 4 s from the start (#112) |
| Transcode | VAAPI output is always H.264, even for HEVC-capable clients (#113 part 2, after #111). The low-power encoders (part 1) are built and tested, not yet checked live |
| IPTV | Some provider feeds stall for 8 s at a time (France 5 on 2026-11-12); the hub reconnects and the player re-tunes, but the picture freezes meanwhile |
| Reviews | IMDb/RT/Metacritic scores need `REEL_OMDB_API_KEY` |
| EPG | Al Jazeera guide fully covered by AE1 (fine). The EG1 guide is stale upstream (removed) |
| IPTV | Live HLS has no quality ladder (live transcode is fixed) |
| IPTV | A VOD remux of a file without a container keyframe index (MPEG-TS, AVI), or from a provider that ignores `Range`, still serves ffmpeg's EVENT playlist and can't be seeked (#92). Transcodes and MKV/MP4 remuxes are seekable |
| Library | Episodes of a multi-episode file all start at 0:00 (no per-episode offsets) |
| Downloads | "Common Side Effects" is stuck in Sonarr ("episodes not imported"). That's a Sonarr-side import problem, not Reel |
| Requests | **Torrent searches fail in bursts, outside Reel** (#66). Root cause: gluetun's DNS-over-TLS to Cloudflare (1.1.1.1:853) times out or gets reset. Prowlarr runs inside gluetun, so every indexer fails at the same moment (`EAI_AGAIN`: 119 of 165 indexer errors from 2026-10-30 to 11-10). The same DoT path feeds gluetun's health check, which caused VPN reconnect loops of 4 h (11-03) and 3 h (11-05). Prowlarr's backoff then reaches Radarr/Sonarr as HTTP 429, and they escalate their own backoff up to 12 h. Requests made in that window search with **0 active indexers**, and Reel marks them `not_found`. There are no Cloudflare/VPN-IP blocks, and Prowlarr↔Arr sync is fine. Fix awaiting owner approval (CT 105): switch gluetun to Proton's in-tunnel DNS, then Retry the 8 `not_found` requests (releases exist for them). Surfacing this in Reel is covered by #67 |
| Toolchain | rustc pinned to 1.86 (some crates pinned for it, e.g. wiremock 0.6.4). sqlx 0.9 needs 1.94, so sqlx stays on 0.8 and its bundled SQLite advisory (RUSTSEC-2026-0459, #80) is open. #16 ruled it not reachable from outside and acceptable for go-live (see Security posture) |

## Security posture

### Re-review 2026-11-15 (#16): GO with conditions

The go/no-go gate for the DNS-only public edge (A39, #32). Three independent reviewers worked from one brief:
Claude Opus 5.5, Claude Sonnet 5.5 and Claude Fable 5.1. GPT wasn't available: the Codex provider is disabled
in the orchestrator, so the council had no non-Claude member this time. The coordinator verified every finding
against the code and dropped the ones it couldn't confirm. The review covered `origin/main` `1b5c1ed`, which is
the live build `1a39b38` plus docs, and `deploy/ct107/reel.caddy`. Live checks were read-only.

**Result: no Critical or High findings.** No auth bypass, IDOR, setup bypass, SSRF, path traversal or ffmpeg
injection was found from the internet. Verified for this review:

- **Edge path blocks.** Caddy 2.11.7, the same build as CT 107, ran locally with PIECE 2 in front of an echo
  backend. Case, percent-encoding, `//`, `./`, `../`, `%2F` and trailing-slash variants of the blocked paths
  get a 403.
- **Variants Caddy lets through.** Some forms pass Caddy: `;` path parameters, a trailing `.`, `%00`, `%20`,
  a backslash, `%5c` and `setup%3F`. Sent straight to Reel, none of them matches a route; they all fall
  through to the SPA. Every API route is under `/api/v1`, with no second mount or alias.
- **Client IP.** Reel on CT 113 sees `10.10.10.73` as the TCP peer, because Docker's DNAT keeps the source.
  It trusts XFF only from that peer, so a client behind the public site gets its real address. That means the
  LAN-only `/setup` check and the login lockout key on the real client.
- **WAN source.** WAN connections reach Caddy `:8443` with their real public source. In 7 days of public access
  logs, the only private source was OPNsense hairpin traffic from the LAN.
- **Information leaks.** Unauthenticated `/api/v1/health` returns only `{"status":"ok"}`, and
  `/api/v1/openapi.json` needs sign-in.
- **#14 on CT 113.** `/appdata` is `0700`; `reel.db`, `-wal`, `-shm` and `signing.key` are `0600`.
- **Slow clients.** Caddy's default read-header timeout cut a slow header off after about 65 s.
- **Blocked admin API in the web app.** A 403 shows "You don't have access to that". It doesn't sign the user out.

**M2 fixes re-checked**, all in the live build: #7, #8, #9, #10, #11, #12, #13, #14 and #28 close their findings.
#15 is partly closed: library links are scoped, but Discover's availability badge and download progress are still
server-wide. That gap is in #128, item 1.

**New tickets** (milestone M2 for the Mediums; all in Backlog):

| Severity | Ticket | Finding |
|---|---|---|
| Medium | #125 | A locked-out client's every login attempt writes an `audit_log` row (no sampling), and the table is never pruned: unauthenticated database growth |
| Medium | #126 | `REEL_ALLOW_DIRECT_LIVE=true` on CT 113 hands the provider URL with its login to any IPTV user, and would do so from the internet |
| Medium | #127 | Admin powers aren't LAN-only. An admin can sign in from the internet with a password alone, and some admin-only routes are outside `/api/v1/admin` (`POST /libraries`, `/libraries/{id}/scan`, `/requests/{id}/approve` and `/decline`, takeover, session kill) |
| Low | #128 | Bundle of hardening items: Discover availability/progress, signed URLs surviving single logout, no default stream cap, WS ticket map flush, invite codes in debug path logs, library paths shown to users, cast URL and image-scope gating, TMDB fetch concurrency, a stale `?token=` doc string |

**Dependencies** (2026-11-15):
- `cargo audit`: one vulnerability, RUSTSEC-2026-0459 (#80). One unsound warning, RUSTSEC-2026-0486 (`rustix`
  1.1.5 xattr functions, via `tempfile`; Reel never calls them, fixed in 1.1.6). The unmaintained `paste`.
  The existing `rsa` ignore.
- `bun audit`: 9 advisories (1 high, 7 moderate, 1 low), all in `vite`, `esbuild` and `launch-editor`. Those are
  dev-server and build tools only. The image serves prebuilt static files, so none of them reach production.
- **#80 ruling: accept, not a blocker.** The CVE needs a crafted `reel.db` plus `-wal` pair with mismatched page
  sizes, opened by Reel. Reel opens only its own database in the `0700` data dir. Writing there already means
  full compromise. Nothing on the network reaches it. #80 stays open for the sqlx 0.9 / rustc 1.94 move, and an
  ignore entry with this reason may go into `server/.cargo/audit.toml`.

**Verdict: GO with conditions** for the Cloudflare DNS-only exposure (#32). All of these must hold before the
public A record and the `:8443` site go live:

1. **#125 fixed and deployed:** sample `login_locked` audits and prune `audit_log`.
2. **#126:** unset `REEL_ALLOW_DIRECT_LIVE` on CT 113, or deploy a fix that restricts direct mode to LAN clients.
3. **#127, minimum:** add method-aware blocks to `@lan_only` in `deploy/ct107/reel.caddy` for `POST /api/v1/libraries*`
   and `POST /api/v1/requests/*/approve|decline`. Give the admin account a password of 16+ random characters.
   The in-Reel rule (no admin powers from non-LAN client IPs) can follow.
4. **Out of Reel's scope, owner-postponed:** rotate the Cloudflare token (still in CT 107's journal).

Recommended, not blocking:
- Explicit `timeouts { read_header 10s idle 2m }` on `servers :8443`. Don't set `write`: it would cut long streams.
- Flip the CSP from report-only to enforced after the DevTools check. With the session token in `localStorage`,
  that's the main XSS mitigation.
- Fix #128.

### First council (2026-11-03)

A three-reviewer council ran on 2026-11-03 (a general reviewer, plus Sonnet and GPT council members). It found
**no Critical issues** and established no auth bypass, SQL injection, path traversal or shell injection.

**Fixed and deployed:**

- Login: Argon2 concurrency slots, failures recorded before verification, (user, IP) lockout, /64 buckets, input caps, sampled audit, bounded limiter.
- First-run `/setup` restricted to the LAN or a one-time token; `trusted_proxies` client-IP trust; signed URLs with `sign_epoch`; 90-day absolute sessions.
- Per-user permissions (IPTV / VOD / requests / casting); DLNA admin-only by default; Arr routing options admin-only.
- Webhooks: secret of 32+ characters, header/Basic auth, constant-time comparison, per-IP lockout, `..` paths rejected.
- WebSocket tickets; sockets closed on revoke; logs no longer contain query strings; safe post-login redirect.

**Fixed and deployed (in the live build; owner verification pending):**

- `/events?token=` removed (#28): the WebSocket accepts only a single-use ticket or an `Authorization` header, so the session token can't land in proxy or edge logs. The SDK (used by the web, webOS and Tizen builds) no longer falls back to `?token=` when the ticket request fails; it retries with backoff.
- Playback errors are generic (stderr redacted server-side); atomic stream/transcode limits; global ffmpeg process budget; single-flight subtitle conversion; no software HEVC.
- **Upstream proxy for ffmpeg/ffprobe** (SSRF policy + credentials out of argv) for VOD, remote subtitles and remote probes.
- Local ffprobe/ffmpeg restricted to `-protocol_whitelist file`; symlink-safe scanning; two-level subtitle filter escaping; scratch-disk budget.
- `stream_guard` decodes the session id (no %-encoded owner-check bypass); `/health` build info and `openapi.json` only for signed-in users.
- Container: memory 3 GB, PIDs 512, CPUs 3; transcode caps.

**The 2026-11-03 items** (all fixed; re-reviewed by #16 above):

1. ~~Upgrade `quick-xml` to ≥ 0.41; sqlx with sqlite only~~ Fixed (#7, ✅ live): `quick-xml` 0.42 (RUSTSEC-2026-0194/-0195 gone) in the XMLTV and DLNA parsers. sqlx was already built SQLite-only; every crate now uses the one workspace entry and CI fails if `sqlx-mysql`, `sqlx-postgres` or `rsa` enter the build graph. `rsa` (RUSTSEC-2023-0071) stays listed in `Cargo.lock` only, because cargo records disabled optional deps, and is ignored with that reason in `server/.cargo/audit.toml`. Verified on `65bd851` (2026-11-11): guide refresh OK, `&` and accents correct, migrations clean. DLNA discovery on the real TV still to check. A fresh `cargo audit` also found RUSTSEC-2026-0459 (bundled SQLite in `libsqlite3-sys` 0.30), tracked in #80: the fix needs sqlx 0.9, which needs rustc 1.94.
2. ~~IPTV/EPG ingestion budgets: bytes, gzip expansion, line/token sizes, record counts, deadlines~~ Fixed (#8, ✅ live): playlist, guide and Xtream API downloads have byte caps and deadlines, gunzipped guides an expanded-size cap, M3U lines and XML events/attributes/texts length caps, channel/programme/VOD/category lists count caps, and downloaded guides are deleted on every exit path. Limits in `server/crates/iptv/src/limits.rs` (ARCHITECTURE A21). Verified on `65bd851` (2026-11-11): a full refresh succeeds (11,950 channels, 41,338 movies, 15,147 series, ~54k programmes), no limit errors, no leftover temp files.
3. ~~Logo/TMDB image fetches: streaming size cutoff, single-flight, cache size budget~~ Fixed (#9, 🧪 deployed in `1a39b38`): bodies are cut off at a byte cap (TMDB 32 MiB, logos/VOD 2 MiB) or deadline (30 s / 10 s) while streaming, concurrent fetches of one image share a single download (the lock map is emptied as fetches finish), failures are remembered briefly, and the TMDB and IPTV image caches have disk budgets with LRU eviction (`REEL_IMAGE_CACHE_MAX_MB`, `REEL_IPTV_IMAGE_CACHE_MAX_MB`; ARCHITECTURE A33). Owner check on the live build pending.
4. ~~Local artwork: canonicalize under the library roots + magic-byte sniff~~ Fixed (#10, 🧪 deployed in `1a39b38`): the image route resolves symlinks, requires a library root, caps size and sniffs magic bytes. Owner check on the real library pending.
5. ~~SSRF guard gaps (IPv4-compatible v6, `64:ff9b::/96`, `2002::/16`, `192.0.0.0/24`); route **live IPTV** HLS upstreams through the upstream proxy~~ Fixed (#11, ✅ live): `net.rs` judges NAT64, 6to4, Teredo and IPv4-translated addresses by the IPv4 address they carry, refuses `::a.b.c.d`, `64:ff9b:1::/48`, `192.0.0.0/24`, `100::/64` and `fec0::/10`, and the guarded client no longer sends the previous (login) URL as `Referer` on redirects. The live hub hands ffmpeg only a loopback proxy URL. Verified on `65bd851` (2026-11-11): no provider login in ffmpeg args (472 samples) or logs. The provider has no HLS channels, so the live-HLS proxy path is covered by CI only. Live TV stalls are tracked in #93.
6. ~~DLNA discovery trust: bind control URLs to the responding IP, cap body sizes, setting to disable discovery~~ Fixed (#12, 🧪 deployed in `1a39b38`): an SSDP `LOCATION` must be on the IP that answered, AVTransport/`URLBase` URLs must stay on the description's host (a foreign volume or icon URL is dropped), descriptions (256 KiB) and SOAP answers (64 KiB) are capped, redirects are not followed, an SSDP answer can't take over a TV added by IP, and `REEL_CAST_DISCOVERY=false` turns SSDP off (ARCHITECTURE A34). Owner check on the live build pending.
7. ~~Live IPTV transcodes counted against the user's `max_transcodes`~~ Fixed (#13, 🧪 deployed in `1a39b38`): a live HLS session whose shared packager re-encodes video counts as one transcode of its viewer (joining a running packager included) and each such packager as one server-wide, together with playback transcodes, under one admission lock. A user's own oldest live transcode gives way when they zap (newest wins); playback transcodes and other users' never do. Passthrough, direct and copy/audio-only HLS don't count. Owner check on the live build pending.
8. ~~Signing key / data dir permissions created atomically (0600/0700)~~ Fixed (#14, 🧪 deployed in `1a39b38`): `signing.key` is written to a new `0600` temp file and renamed into place, the data dir and any parent Reel creates are `0700`, and a new `reel.db` is created `0600` before SQLite opens it (SQLite gives `-wal`/`-shm` the same mode). At startup the data dir is tightened to `0700` and the key, `reel.db`, `-wal`, `-shm` and `-journal` to `0600` (owner bits kept; a file the process doesn't own is logged, not fatal). CT 113 before the fix (2026-11-15): everything owned by uid 1000 (the container's `reel` user); data dir `0755`, `reel.db`/`-wal`/`-shm` `0644`, `signing.key` already `0600`, 3,586 world-readable entries. Checked on CT 113 by #16 (2026-11-15): `/appdata` `0700`; `reel.db`, `-wal`, `-shm` and `signing.key` `0600`.
9. ~~Redact `epg_urls`/source URL credentials in admin DTOs; filter `library_item_id` in Discover/Downloads by library visibility~~ Fixed (#15, 🧪 deployed in `1a39b38`): admin source responses mask the source URL, guide URLs and the Xtream username; edits accept the masked values back, refuse a URL that still holds `***`, and need the login again to move an Xtream source to another server. Discover (lists, search, detail), person pages, global search and Downloads link only a copy of a title in a library the viewer can open. Owner check on the live build pending.
10. Edge, if ever public: only Caddy 443 forwarded; block `/api/v1/{admin,setup,webhooks,cast}/*` from the WAN; CSP/Permissions-Policy at Caddy; rate limits/CrowdSec; geo allowlist. **Prepared, not applied** (#31): a reviewed Caddy site for the public `:8443` listener (`deploy/ct107/reel.caddy`, checked with Caddy 2.11.7: path blocks incl. encoded/dot/double-slash variants, XFF overwrite, headers, no h3), a merge script, the runbook and an external check script (OPERATIONS → Public edge). Reel now takes `REEL_TRUSTED_PROXIES=10.10.10.73` in plain form (it used to refuse to start), logs whose XFF it believes, and `edge_*` tests show a spoofed XFF can't open the LAN-only setup or dodge the login lockout. Decision: Cloudflare DNS-only (A39). Not done: Caddy rate limiting (module not installed; Reel's lockout covers login), geo allowlist (optional, whole public listener), CSP still report-only. Applied 2026-11-15: CT 113 trusts only Caddy (`REEL_TRUSTED_PROXIES=10.10.10.73`, the any-LAN-peer `REEL_TRUST_PROXY_HEADERS` alias is gone) and the #108 h3 part of the Caddy change; the public `:8443` site is not applied. Go-live is #32.

**Decision on record:** Reel stays **private** (LAN + Tailscale) until the #16 conditions above (#125, #126, #127 minimum) are met; then #32 may apply the public edge.

## Backlog (next)

1. Playback like YouTube (#102 proposal, awaiting owner approval): quality picker with a per-device default (#87),
   then remove the admin's 6 Mbps cap (#2); multi-variant HLS with a lazy low rung and hls.js ABR (#111, replaces
   the closed PR #69's re-decide stepping, #1); faster first frame (#112); low-power/HEVC encodes (#113); HDR in the browser
   (#114); optional background pre-encode (#115).
2. Meet the #16 go-live conditions (#125, #126, #127), then #32; then #128.
3. Verify webOS/Tizen on real TVs; DLNA with the TV IPs; AirPlay/Chromecast.
4. IPTV VOD playback check on the provider (#3, owner); live HLS quality ladder.
5. OMDb key for IMDb/RT scores.
6. Trickplay thumbnails (the scrubber already has the slot), intro/credits skip.
7. Android TV / phone app: epic #29 with #185–#196 (milestone M5). The skeleton and CI job are merged (#185), and so are signed release APKs on an `android-v*` tag (#195); the owner still has to create the release key and set the four `ANDROID_*` Actions secrets before the first tag. Connect + sign-in is merged (#186). The app shell is merged (#187). Browse is in review (#188): Home, Library (paged grid, sort, filters) and item detail (play / resume / mark watched, seasons and episodes), checked on a phone and an Android TV emulator against a mock API (D-pad through rail, rows, grid paging and detail; focus restored on Back), not yet on the live server or a real device. Search is in review (#189, stacked on #188): checked on phone and TV emulators against a mock API (the keyboard opens at once on TV; its search key moves to the first result). Settings is in review (#194, stacked on #189): checked on phone and TV emulators against a mock API. The VOD player is in review (#190): on a phone and an Android TV emulator against a mock API, an H.264 MP4 and an H.264 MKV (AC3 + AAC + SRT) direct-played with resume, sidecar subtitles, local audio switching, progress, 404 recovery, quality re-decide, up next and remote keys; HLS played; the emulators can't decode HEVC Main 10 or AC3, so the HEVC 10-bit MKV went through *Play transcoded*. Not yet tried against the live server or on a real phone (#196; the owner has no Android TV, so TV is emulator-only). IPTV VOD is in review too (#193), checked on both emulators against the mock only (lanes, paged grid, movie and series pages, resume, the 429 "your other screen" stop and the 409 takeover, slot released on leave), never against the live provider (one connection, rate limits). Android Live TV is built (🧪 #191): local tests cover guide and favourites, rapid zaps, takeover, and session release on zap, leave and background. Real-phone playback and warm-channel zap timing against the provider remain to verify with #196. The EPG grid is tracked in #192. iOS later.
   Reel Web for Android (epic #225, #226–#232): a WebView client to compare against the Compose app; the shell (#226), Back/full screen (#227), web identity (#229) and the TV check (#231, emulator) are built; recovery (#228), the WebView playback matrix (#230) and Obtainium releases (#232) are open.
8. Per-episode offsets for multi-episode files.

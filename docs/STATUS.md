# Status: 2026-11-11

Live build: **`99b0deb`** (2026-11-04 09:09 UTC) on CT 113 → https://reel.rafikii.co (LAN + Tailscale only).

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
- **Ops:** GitHub → GHCR → CT 113 deploys (one-click **deploy** workflow on the self-hosted runner CT 116; the `PROJECT_TOKEN` secret is needed for ticket updates); version shown in Settings → About; container resource limits; transcode caps (3 total, 2 per user).
- **Network:** Tailscale now **direct** (UDP 41641 static-port rule on OPNsense) → 9–15 Mbps remote, up from 3.5 Mbps over the DERP relay.

## Built but not exercised for real (🧪)

- **webOS/Tizen apps:** never installed on a TV. Packaging CLIs aren't installed; the Tizen package id is a placeholder.
- **Casting:** AirPlay and Chromecast never tested with devices. **DLNA** needs the TV IPs + a CT 113 firewall rule.
- **IPTV VOD playback** through the upstream proxy, against the real provider.
- **Direct (local) Live TV playback** (#77): built and tested in CI, not yet tried on a TV or against the real provider. Off until `REEL_ALLOW_DIRECT_LIVE` is set on CT 113.
- **Notifications** (Discord/Telegram/webhook).

## Known gaps / bugs

| Area | Gap |
|---|---|
| Player | **Adaptive quality client** (bandwidth estimate, auto step-down/up, quality menu) wasn't delivered (the agent errored). For now, set per-user `max_bitrate_kbps` (the admin is at 6000) |
| Reviews | IMDb/RT/Metacritic scores need `REEL_OMDB_API_KEY` |
| EPG | Al Jazeera guide fully covered by AE1 (fine). The EG1 guide is stale upstream (removed) |
| IPTV | Live HLS has no quality ladder (live transcode is fixed) |
| Library | Episodes of a multi-episode file all start at 0:00 (no per-episode offsets) |
| Downloads | "Common Side Effects" is stuck in Sonarr ("episodes not imported"). That's a Sonarr-side import problem, not Reel |
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

1. ~~Upgrade `quick-xml` to ≥ 0.41; sqlx with sqlite only~~ Fixed in code (#7, 🧪): `quick-xml` 0.42 (RUSTSEC-2026-0194/-0195 gone) in the XMLTV and DLNA parsers. sqlx was already built SQLite-only; every crate now uses the one workspace entry and CI fails if `sqlx-mysql`, `sqlx-postgres` or `rsa` enter the build graph. `rsa` (RUSTSEC-2023-0071) stays listed in `Cargo.lock` only, because cargo records disabled optional deps, and is ignored with that reason in `server/.cargo/audit.toml`. Counts as done once deployed and an EPG refresh and a DLNA discovery have been checked. A fresh `cargo audit` also found RUSTSEC-2026-0459 (bundled SQLite in `libsqlite3-sys` 0.30), tracked in #80: the fix needs sqlx 0.9, which needs rustc 1.94.
2. IPTV/EPG ingestion budgets: bytes, gzip expansion, line/token sizes, record counts, deadlines.
3. Logo/TMDB image fetches: streaming size cutoff, single-flight, cache size budget.
4. ~~Local artwork: canonicalize under the library roots + magic-byte sniff~~ Fixed in code (#10, 🧪): the image route resolves symlinks, requires a library root, caps size and sniffs magic bytes. Counts as done once deployed and checked on the real library.
5. SSRF guard gaps (IPv4-compatible v6, `64:ff9b::/96`, `2002::/16`, `192.0.0.0/24`); route **live IPTV** HLS upstreams through the upstream proxy.
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
4. IPTV VOD playback check on the provider; live HLS quality ladder.
5. OMDb key for IMDb/RT scores.
6. Trickplay thumbnails (the scrubber already has the slot), intro/credits skip.
7. Android TV / phone app (needs the Android SDK), iOS later.
8. Per-episode offsets for multi-episode files.

# Operations

> **No secrets in docs, shells, or chat.** Credentials live in root-only files on the hosts listed below.
> Use hidden prompts (`read -rs`) to set them.

## Topology

| Piece | Where | Notes |
|---|---|---|
| Reel container | Proxmox `minix` → **CT 113** (`plex`, 10.10.10.207) | `/root/reel/docker-compose.yml` + `.env` (root-only). Shares the iGPU (`/dev/dri`) and the read-only media mount with Plex |
| Data | CT 113 `/opt/appdata/reel` → container `/appdata` | `reel.db` (SQLite WAL), `signing.key`, image/transcode caches. Owned by uid 1000 (the container's `reel` user); Reel keeps the dir `0700` and the key and DB files `0600` (tightened at startup, #14), so other readers need uid 1000 or root |
| Media | `/mnt/media/media` → container `/data` (ro) | Same paths as Radarr/Sonarr (`/data/media/...`), so webhooks need no path mapping |
| TLS / name | Caddy **CT 107** (10.10.10.73, Caddy 2.11.7 + `dns.providers.cloudflare` only), LAN block `*.rafikii.co` on :443 → `@reel host reel.rafikii.co` → `10.10.10.207:8420` (`flush_interval -1`, `Alt-Svc: clear`) | LAN only. Backups: `/etc/caddy/Caddyfile.pre-reel-*` (h3 change: `Caddyfile.pre-reel-h3-20261115T034930Z`). CT 107 also has a **public listener :8443** (WAN TCP/443 is DNATed there) for immich, ha and plex, with h3 off (`servers :8443 { protocols h1 h2 }`); Reel has no site on it yet. Unit: drop-in `/etc/systemd/system/caddy.service.d/no-environ.conf` replaces the packaged `caddy run --environ` (it printed the Cloudflare token to the journal) with plain `caddy run`; `override.conf` loads `/etc/caddy/cloudflare.env`. Ready-to-apply Reel edge: [`deploy/ct107/reel.caddy`](../deploy/ct107/reel.caddy), runbook [below](#public-edge-caddy-ct-107) |
| DNS | Technitium `10.10.10.2` zone `reel.rafikii.co` → A `10.10.10.73` | API token at `/root/.technitium-token` on minix |
| Arr stack | **CT 105** (10.10.10.122): Radarr :7878, Sonarr :8989, Sonarr Anime :8990, Prowlarr :9696, qBittorrent :8200 | Each Arr has a "Reel" webhook connection. Compose: `/mnt/stacks/docker-compose-{vpn,media}.yaml` (project `mediastack`). **Only Prowlarr** runs inside gluetun's network namespace (Proton WireGuard, NL); Radarr/Sonarr reach it at `10.10.10.122:9696`. qBittorrent has been on the plain `mediastack` network (direct WAN) since 2026-10-30, a temporary exception the owner asked for (see the comment in the compose file) |
| Firewall | CT 113 Proxmox firewall drops LAN egress apart from the Arr ports and **qBittorrent :8200** (added for Reel) | Backup: `/root/113.fw.bak-*` |
| Remote access | Tailscale; OPNsense has a static-port outbound NAT + WAN UDP 41641 pass (Firewall → Automation, "Tailscale: …") | Config backup `/conf/backup/config-pre-tailscale-static-port-20261103T165802Z.xml` |
| Image registry | `ghcr.io/mrafik92/reel` (private) | CT 113 is logged in with a **pull-only** PAT (`read:packages`) |
| Deploy runner | **CT 116** `gh-runner` (10.10.10.196, Debian 13, unprivileged, 512 MB) | GitHub Actions self-hosted runner `gh-runner-ct116`, label `homelab`, service user `runner`. Outbound only: firewall `policy_in DROP`, LAN egress limited to DNS/NTP, minix:22, Caddy:443 |
| Deploy key | minix `/root/.ssh/authorized_keys` → `restrict,command="/usr/local/bin/reel-deploy",from="10.10.10.0/24"` | `reel-deploy` accepts only `sha` / `deploy` and logs to the journal (`journalctl -t reel-deploy`). Host firewall `host.fw`: `IN ACCEPT -source 10.10.10.196 -p tcp -dport 22` (backup `/root/host.fw.bak-*`) |

### Credentials and where they live

| Secret | Location |
|---|---|
| Reel admin (`rafik`) | minix `/root/.reel-admin` (root-only). Change the password in Settings |
| TMDB key, webhook secret, (OMDb, Trakt) | CT 113 `/root/reel/.env` |
| IPTV provider login, qBittorrent password, Arr API keys | Reel DB (entered in Settings) |
| `PROJECT_TOKEN` (classic PAT: `repo`, `project`) | GitHub repo secret, used by the `deploy` workflow to comment on tickets and move them to Deployed |
| Cloudflare API token for Caddy DNS-01 (`CF_API_TOKEN`: zone rafikii.co, Zone Read + DNS Edit) | CT 107 `/etc/caddy/cloudflare.env` (root, 0600), loaded by the caddy unit's `override.conf`. Was written to the journal by `--environ` until 2026-11-15; **rotation pending** (owner deferred it), see Public edge → Token rotation |
| Android release signing (#195, #232): keystore `reel-release.jks` (alias `reel`, certificate SHA-256 `a4865475…58ecb`) and its password, for both Android apps | Mac `~/.reel-android/reel-release.jks` (dir 0700, file 0600); the keystore file and its password are backed up in the owner's password manager (the only backup: losing the key means future APKs can't update installed apps). `scripts/android-release.sh` reads the password from `REEL_KEYSTORE_PASSWORD`, or the macOS Keychain item service `reel-android-keystore`, account `reel` (optional; add it once with `security add-generic-password -s reel-android-keystore -a reel -w`), or a hidden prompt; never from a file. Also the GitHub Actions secrets `ANDROID_KEYSTORE_B64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`, used only by `android-release.yml` (not in use while Actions is unavailable). A git-ignored `clients/android/keystore.properties` also works for plain Gradle but puts the password on disk. Setup: `clients/android/README.md` → Release builds |
| OPNsense API key (firewall automation) | Mac `~/.config/opnsense-api` (0600). Revoke in OPNsense if no longer needed |

## Build & deploy pipeline

```
git push main ──► GitHub Actions
                   ├── ci.yml:    cargo fmt/clippy/nextest (rustc 1.86, ffmpeg, mold), bun typecheck/build
                   └── image.yml: docker buildx (deploy/Dockerfile) → ghcr.io/mrafik92/reel:latest + :sha-xxxx
                                  build args GIT_SHA / BUILD_DATE → shown in Settings → About
CT 113:  Actions → deploy → Run workflow  (runner CT 116 → ssh forced command → compose pull/up/prune)
```

- CI runs **once** per PR (`pull_request`) and once per push to `main`. The `server` job is skipped on PRs
  that touch neither `server/**` nor `ci.yml` (the `clients` and `docs` checks still run). Tests run with
  `cargo nextest run --workspace --profile ci` (parallel across test binaries; no doctests exist), debug info is off
  (a CARGO_HOME config written by the job) and linking uses mold. The Rust cache (`shared-key: reel`) is
  saved only from `main`; PRs restore it read-only. Locally, plain `cargo test --workspace` still works.
- The runner has 2 vCPUs. The `ci` nextest profile (`server/.config/nextest.toml`) runs 4 tests at once,
  and the ffmpeg-heavy playback binaries take 2 slots each, so tests that wait on timers overlap with
  CPU-bound ones without more ffmpeg contention than before.
- Playback tests build their media through `tests/common::ffmpeg_fixture`, once per distinct ffmpeg command
  and ffmpeg version, into `server/target/tmp/reel-fixtures` (shared by all tests of a build; each test gets
  its own copy). CI keeps that directory in an Actions cache (`reel-fixtures-…`, saved from `main` only,
  unused fixtures pruned before saving) and removes it from `target/` so the Rust cache doesn't carry it.
- ffmpeg's `.deb`s are cached per runner image (`apt-ffmpeg-<ImageOS>-<ImageVersion>`, saved from `main`)
  and installed with `dpkg -i`, falling back to `apt-get` on a miss or any error. The OpenAPI check runs
  `target/debug/export-openapi`, which the test build already linked (`cargo run` would rebuild with the
  non-test feature set).
- No sccache: it can only cache the workspace library crates (~25 s of compile; `reel-api`, the biggest,
  changes in most PRs). Test binaries and linking, the other ~95 s, can't be cached, and dependencies
  already come from the Rust cache.
- `image.yml` keeps its Docker layer cache in the registry (`ghcr.io/mrafik92/reel:buildcache`), not the
  Actions cache, so it can't evict the Rust cache (10 GB repo limit). The Dockerfile builds dependencies
  with cargo-chef in their own layer, keyed by `Cargo.toml`/`Cargo.lock` only, so a server change rebuilds
  just the workspace crates. BuildKit cache mounts don't help there, because each runner starts empty.
- Server changes take ~10 min to build; web-only changes take ~1 min (cached Rust layer).
- `image.yml` cancels in-flight builds on new pushes, so wait for the **latest** run.
- Deploy (from the Mac):
  ```sh
  ssh minix-local 'pct exec 113 -- sh -c "cd /root/reel && docker compose pull -q && docker compose up -d && docker image prune -f"'
  curl -s https://reel.rafikii.co/api/v1/health   # {"status":"ok"} (version only when signed in)
  ```
- **Preferred: GitHub.** After an image build, its run summary links to **Actions → deploy → Run workflow**. That click is the gate (web or the GitHub mobile app). The job runs `deploy/ct113/deploy.py --forced` on CT 116: it deploys, health-checks, moves tickets to Deployed and writes a run summary. Runs are serialized, and the job refuses to deploy while a newer image build is still running. Recovery: run it with **since = <old sha>** to (re)process tickets without deploying.
- **Fallback from the Mac:** `deploy/ct113/deploy.py` (or `--dry-run`). It pulls the latest successful image, restarts, runs a health check, then finds tickets referenced in the commits since the previously deployed build. Each gets a comment "🚀 Deployed to CT 113 — build `abc1234`" and moves to **Deployed** on the project board.
- **Android app releases** (#232) are separate from the server image and, while GitHub Actions is unavailable
  (account billing), built on the Mac for both apps:
  ```sh
  scripts/android-release.sh web     --taildrop     # Reel Web  → dist/ReelWeb-X.Y.Z.apk (+ .sha256)
  scripts/android-release.sh compose --taildrop     # Compose   → dist/Reel-X.Y.Z.apk    (+ .sha256)
  ```
  It runs lint and unit tests, builds a signed, R8-shrunk APK (versionName `X.Y.Z`, versionCode
  `X*1_000_000 + Y*1_000 + Z`; default: the next patch after the app's newest tag), checks that it carries the
  release certificate, and copies it to the git-ignored `dist/`. `--taildrop` sends it to the phone `rafiiiki`
  with `/Applications/Tailscale.app/Contents/MacOS/Tailscale file cp` (the `tailscale` shell alias isn't
  available in scripts). An offline phone ("reportedly offline", 502 Bad Gateway) is reported plainly, with the
  command to send it later, and the script exits 3. `--github-release` (opt-in) tags HEAD (`android-web-vX.Y.Z`
  or `android-vX.Y.Z`), pushes the tag and runs `gh release create`, which works without Actions; Obtainium
  filters per app are in the two READMEs. `--dry-run` builds but only prints the Taildrop and GitHub commands.
  Passwords: see Credentials. `android-release.yml` (#195, built `android-v0.1.0`–`0.3.0` on a pushed tag) stays
  in the repo but isn't in use. Sideloading: `clients/android/README.md`.
- Rollback: pin `image: ghcr.io/mrafik92/reel:sha-<previous>` in the compose file and run `up -d`.

### Ticket flow (GitHub Project #1)

Backlog → To do → Implementing → Reviewing → **Deployed** (automatic, `deploy.py`) → Verified (manual, fill *Verified on*) → Done (close the issue).

- Reference tickets in commits with `Refs #12`. Use `Fixes #12` only when no real-setup verification is needed; GitHub closes it on push, skipping Deployed/Verified.
- Built-in project workflows (Project → ⋯ → Workflows): *Item added → Backlog*, *Item closed → Done*, *Auto-close issue (Status = Done)*, *Auto-add to project* (`is:issue is:open` on `mrafik92/reel`; issues only, PRs aren't cards). The agent moves a card to **Reviewing** when it opens the PR (`Refs #N` doesn't create a GitHub link, so *Pull request linked to issue* doesn't fire).

## Configuration (CT 113 compose → env)

| Var | Value | Purpose |
|---|---|---|
| `REEL_DATA_DIR` | `/appdata` | DB, caches, signing key |
| `REEL_PUBLIC_URL` | `https://reel.rafikii.co` | Invite links; never derive URLs from Host |
| `REEL_CAST_BASE_URL` | `http://10.10.10.207:8420` | Absolute LAN URLs for TVs/Chromecast |
| `REEL_ALLOW_DIRECT_LIVE` | unset (`false`) | Lets devices play Live TV straight from the provider (opt-in per device). Turning it on sends the provider login (it is part of the stream URL) to those devices. On CT 113 it goes in `/root/reel/.env` (the compose loads it with `env_file`) |
| `REEL_MAX_TRANSCODES` / `REEL_MAX_USER_TRANSCODES` | `3` / `2` | Transcode caps, shared by playback and live TV: a live channel whose video is re-encoded counts once per viewer for the user and once per channel server-wide |
| `REEL_TRUST_PROXY_HEADERS` | `true` | Deprecated: believes `X-Forwarded-For` from **any** private-range peer, so any LAN host can choose its client IP. Logs a warning at startup. **Not set on CT 113** since 2026-11-15 (replaced by `REEL_TRUSTED_PROXIES`) |
| `REEL_TRUSTED_PROXIES` | `10.10.10.73` | Proxies whose `X-Forwarded-For` is believed (comma-separated for more): Caddy CT 107 only. The client is the right-most XFF entry that isn't a trusted proxy. Startup logs `client IP taken from X-Forwarded-For of these proxies proxies=["10.10.10.73"]`. Set since 2026-11-15 (build `1a39b38`; older builds only took `[10.10.10.73]`). Compose backup: `/root/reel/docker-compose.yml.pre-trusted-proxies-20261115T034742Z` |
| `REEL_MAX_TRANSCODES` / `REEL_MAX_USER_TRANSCODES` | `3` / `2` | Transcode caps |
| `REEL_ABR` | unset (on) | Multi-variant HLS for players that ask for it (`adaptive: true`, hls.js; #111): the original plus lazily transcoded rungs. `0`/`false` serves one rendition per session everywhere, the rollback lever without a rebuild |
| `REEL_ABR_VARIANT_IDLE_SECS` | unset (`30`) | A variant nobody fetched for this long has its ffmpeg stopped and its transcode slot freed (minimum 5) |
| `REEL_VAAPI_LOW_POWER` | unset (on) | `0`/`false`/`off`/`no` skips the startup low-power probe, so VOD transcodes use the regular VAAPI encoders (#113). Admin → Hardware shows what's in use; the probe results are in `notes` of `GET /api/v1/admin/hwaccel` |
| `REEL_ALLOW_PRIVATE_FETCH` / `..._LOGO_FETCH` | `true` / `false` | SSRF policy (playlists/EPG may be LAN; logos never) |
| `REEL_IMAGE_CACHE_MAX_MB` / `REEL_IPTV_IMAGE_CACHE_MAX_MB` | unset (`4096` / `1024`) | Disk budgets of the TMDB image cache (`/appdata/images`) and the IPTV logo + VOD image caches (`/appdata/iptv/{logos,vod-images}`). Beyond them the least recently used files are deleted (log line `image cache over budget`). `0` = unlimited. Live sizes on 2026-11-15: 772 MB and 62 MB |
| `REEL_CAST_DISCOVERY` | unset (`true`) | DLNA SSDP discovery every minute. `false` = only TVs added by IP (Settings or `POST /api/v1/admin/cast/devices`) are used; they are kept in `/appdata/cast_devices.json`. Discovered TVs must answer from the same IP their description and control URLs use (#12) |
| `REEL_TMDB_API_KEY`, `REEL_WEBHOOK_SECRET` | in `.env` | |
| `REEL_OMDB_API_KEY`, `REEL_TRAKT_CLIENT_ID` | optional | IMDb/RT scores, Trakt comments |
| `REEL_WS_PING_SECS` | unset (`25`) | Ping interval of the `/events` WebSocket, so idle proxy/VPN paths don't drop it silently. `REEL_WS_RECHECK_SECS` (`30`) re-checks the session on open sockets |
| Container limits | `mem_limit 3g`, `pids_limit 512`, `cpus 3.0`, `read_only`, `cap_drop ALL`, `no-new-privileges` | |

## Runbooks

**Logs**
```sh
ssh minix-local 'pct exec 113 -- docker logs --since 30m reel 2>&1 | tail -100'
```
To debug decisions, Settings → Now playing shows each session's mode, reasons, bitrate and HW backend.

**Stuttering / buffering**
1. Settings → Now playing: what's the mode and bitrate?
2. Measure the client's throughput (a big ranged `curl` of a file URL).
3. If the bitrate is above the throughput: lower the user's `max_bitrate_kbps` (Settings → Users) until the player has auto quality.
4. Tailscale clients: `tailscale status` should show `direct`, not `relay`.
5. The web player recovers by itself ("Reconnecting…" overlay, then it reopens the session or re-tunes once). If
   it still ends on "Playback stopped", capture the browser console and network tab. Chrome on Tailscale may use
   HTTP/3 (Caddy Alt-Svc); `ERR_QUIC_PROTOCOL_ERROR` / status-0 responses point at QUIC over the 1280-byte MTU.
   A console line `[player] stalled at … s: segment N requested … ago and still loading` (or `the playlist
   requested … ago and still loading`) means a request hung in flight. Since 2026-11-15 Caddy sends `Alt-Svc: clear`
   for reel.rafikii.co, so browsers should use HTTP/2; check DevTools → Network → Protocol shows `h2`, not `h3` (#108).
6. A paused converted (transcode/remux) session stays for 30 min after its last "paused" ping; others are reaped
   after 60 s without requests or pings (`reaping idle playback session` in the logs). The web player reopens a
   reaped session by itself at the same position.

**Live TV stops or freezes**
Browser live HLS keeps a 30 s window (2 s segments) plus ~20 s of rotated-out segments on disk (`hls.rs`,
`LIVE_*` constants). Check `GET /api/v1/admin/iptv/hub` and the logs: `upstream failed: stalled` means the
provider feed stalled (the hub reconnects); 404s for `seg_*.m4s` in the browser mean the viewer fell more than
~50 s behind.

**IPTV provider blocks (HTTP 46x)**
The panel's anti-abuse limit. Don't hammer it; Reel backs off automatically. Check the source status in Settings → Live TV.
A single-connection plan means only one stream at a time; your own newest stream wins.

**TV guide**
Settings → Live TV → Providers → Guides: ordered list, per-guide stats, "Suggested guides" (epgshare01). Use
**Refresh guide** after changes. A per-channel EPG override is in the channel manager.

**qBittorrent**
URL must be `http://10.10.10.122:8200` (the CT 113 firewall allows only that). qBittorrent 5 cookies (`QBT_SID_*`) are supported.

**Torrent search finds nothing / requests end in `not_found`** (analysis: #66)
All read-only. Run from the Mac: `ssh minix-local` → `pct exec 105 -- …`. Never print API keys; read them inside
the container (`sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml`).
1. **Radarr/Sonarr health:** `GET /api/v3/health` and `GET /api/v3/indexerstatus` (`disabledTill` per indexer).
   "All search-capable indexers are temporarily unavailable" means searches run with **0 active indexers**, and
   Reel marks those requests `not_found` even though releases exist.
2. **Prowlarr:** `GET /api/v1/indexerstatus` and the log (`/config/logs/prowlarr.txt`). When Prowlarr has backed
   off an indexer, it answers Radarr/Sonarr with **HTTP 429** and a `Retry-After`. The Arr then disables the
   "(Prowlarr)" indexer for that long and escalates its own backoff (1 min → 5 → 15 → 30 min → 1 h → 3 h → 6 h → 12 h).
3. **Read the exception, not the summary.** Prowlarr logs "Unable to connect to indexer … DNS/SSL issues" for
   every network error. The stack trace after it tells them apart. `Resource temporarily unavailable` from
   `Dns.GetHostEntry…` means **gluetun's DNS timed out**. An HTTP 5xx (`521`, `500`) means the site's origin is down.
4. **gluetun:** `docker logs --timestamps gluetun`. Look for `[dns] … TLS handshake with 1.1.1.1:853 … connection
   reset by peer` (DNS-over-TLS to Cloudflare failing) and `restarting VPN because it failed to pass the
   healthcheck` loops. The health check resolves `github.com`/`cloudflare.com` through the same DoT path, so a
   DoT outage makes gluetun reconnect every ~6 s.
5. **Time zones differ:** Prowlarr/Radarr/Sonarr log files use local time (CET, `TZ=Europe/Amsterdam`). Docker
   `--timestamps` and the Prowlarr history API use UTC.
6. **DNS from inside the VPN:** `docker exec prowlarr getent ahostsv4 nyaa.si`, and compare
   `nslookup <name> 127.0.0.1` (gluetun DoT) with `nslookup <name> 10.2.0.1` (Proton's resolver inside the tunnel).
7. After the cause is fixed: Reel → Requests → **Retry** on each `not_found` request. Radarr doesn't search old
   missing movies again by itself; RSS only picks up new releases.

**Adding a DLNA TV** (pending)
1. Get the TV's IP and model.
2. CT 113 firewall: allow TCP out to the TV's UPnP port (Samsung 9197; LG somewhere in 1000–2000), plus TCP in from the TV to 8420.
3. `POST /api/v1/admin/cast/devices {"ip": "<tv>"}`, or use Settings.
4. If SSDP finds the TV but it doesn't show up, check the logs for `DLNA description points away from the device` (its control URLs are on another address) and add it by IP instead. To rely on IP-added TVs only, set `REEL_CAST_DISCOVERY=false` in `/root/reel/.env`.

**Deploy runner (CT 116)**
- Status: GitHub → Settings → Actions → Runners (`gh-runner-ct116` online), or `systemctl status 'actions.runner.*'` in CT 116.
- Re-register: `gh api -X POST repos/mrafik92/reel/actions/runners/registration-token`, then `./config.sh --url … --token … --labels homelab --replace` as `runner` in `~/actions-runner`.
- If the runner's IP changes from 10.10.10.196, update `host.fw` and `116.fw` (the key's `from=` allows 10.10.10.0/24).
- **The repo must stay private** while this runner is attached.

**Bootstrap a fresh instance**
`deploy/ct113/bootstrap.py` (on minix) creates the admin, libraries, Arr servers and webhooks. It's idempotent and never prints keys.

## Public edge (Caddy CT 107)

**Status:** steps 1 and 2 are **applied** (2026-11-15): Reel trusts only Caddy, HTTP/3 is off for Reel, and the
public site (PIECE 2, including the #127 `@lan_only_writes` block) is live on CT 107 `:8443`. The #16 re-review
(2026-11-15) gave **GO with conditions**; the owner accepted the open Medium items (#125, #126, #127's in-Reel
part) as low impact and chose to go live. Step 3 (the public DNS record, #32) is the owner's.

**Why:** abroad, the owner's mobile carrier throttles Tailscale's UDP to ~3–6 Mbit/s, while plain HTTPS
gets ~47 Mbit/s.

### Design

```
internet ──TCP 443──► OPNsense WAN (DNAT, exists) ──► Caddy CT 107 :8443 ──► Reel CT 113 :8420
LAN / Tailscale ──► Technitium: reel.rafikii.co = 10.10.10.73 ──► Caddy :443 (LAN block, unchanged access)
```

- **Cloudflare DNS-only (grey cloud)**, like immich and plex today. Reasons are in ARCHITECTURE A39. The
  public A record is the home IP `85.144.240.8`; no AAAA (CT 107/113 have no global IPv6).
- **Same name, two doors.** Technitium keeps answering `10.10.10.73` on the LAN and to Tailscale clients,
  so they keep the full app, admin included. Only clients that resolve the public record get the
  restricted `:8443` site.
- **Blocked on the public site** (403 JSON `{"error": …}` from Caddy): `/api/v1/admin/*`, `/api/v1/setup`
  except `GET` (the web app reads `needs_setup` on every anonymous load; blocking it shows "Can't reach
  your Reel server"), `/api/v1/webhooks/*` and `/api/v1/cast/*`. **What stops working from outside:** every
  server section of Settings (Users, Invites, Libraries, Live TV, Requests setup, Now playing, Audit),
  "Play on…" to home TVs, and first-run setup. Playback, Live TV, requests, invites (`/invite/…`), personal
  Settings → Playback and the `/events` socket all work. Admin work stays on the LAN or Tailscale.
- **Client IP.** Caddy's `:8443` has no `trusted_proxies`, so `{client_ip}` is the TCP peer, and Caddy
  **overwrites** `X-Forwarded-For` / `X-Real-IP` with it and drops `Forwarded`. Reel believes those headers
  only from `10.10.10.73` (`REEL_TRUSTED_PROXIES`). Proven by `edge_*` tests in
  `server/crates/api/tests/security.rs`: no header combination from an internet client opens the LAN-only
  setup, and a spoofed prefix doesn't escape the login lockout.
- **Headers:** HSTS, CSP (**report-only** first), Permissions-Policy (only features Reel never uses),
  no `Server`, `Alt-Svc: clear`. Reel adds `nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`.
- **HTTP/3 (#108):** off on `:8443` (`servers :8443 { protocols h1 h2 }`; UDP 8443 is not forwarded
  anyway). On the LAN `:443`, Caddy can't turn h3 off per site, so Reel's handler sends `Alt-Svc: clear`
  instead of `h3=":443"`, which also clears the 30-day h3 entry browsers cached.
- **Rate limits:** CT 107's Caddy has **no** `caddy-ratelimit` and no CrowdSec. OPNsense runs CrowdSec
  (`os-crowdsec`, firewall bouncer, community blocklists `crowdsec_blocklists`) but it doesn't read Caddy's
  logs. Reel's own lockout (per IP with /64 buckets, per (user, IP), exponential) covers login once client
  IPs are real. Caddy rate limiting is optional (commented block in the snippet; needs
  `caddy add-package github.com/mholt/caddy-ratelimit`).
- **Logs:** `import public-access-log reel` → `/var/lib/caddy/logs/reel-public-access.json`, with URIs and
  headers deleted (signed URLs, tokens and tickets never reach disk).

The snippet was checked locally with Caddy 2.11.7 (same as CT 107): `caddy validate`, the blocked and
allowed paths (also `/API/V1/ADMIN`, `%61dmin`, `//admin`, `./`, `../`, `admin%2F`), header overwriting,
no UDP listener on :8443, `Alt-Svc: clear` on an h3-enabled listener, and the access-log filter. The CSP
was checked **enforced** in Chromium: the built web app in mock mode (home, search, live, item, player,
settings) and hls.js with its worker playing fMP4 HLS through `blob:`, with no violations.

### Steps (each with its rollback)

1. **Reel trusts only Caddy** (CT 113 compose, after this PR's build is deployed). In
   `/root/reel/docker-compose.yml`, replace `REEL_TRUST_PROXY_HEADERS: "true"` with
   `REEL_TRUSTED_PROXIES: "10.10.10.73"`, then `docker compose up -d`. Check the log line
   `client IP taken from X-Forwarded-For of these proxies` and a login from the LAN (audit shows the
   client's IP, not 10.10.10.73). Rollback: put the old line back and `up -d`.
   Can be done now, independent of going public.
   **Applied 2026-11-15.** Backup `/root/reel/docker-compose.yml.pre-trusted-proxies-20261115T034742Z`. Checked:
   health ok, the log line names `10.10.10.73` with no warning, and failed logins through
   https://reel.rafikii.co were audited as `10.10.10.10` (minix) and `10.10.10.1` (a Tailscale client, routed),
   not `10.10.10.73`. Rollback: `cp -p` that backup over `docker-compose.yml`, then `docker compose up -d reel`.
2. **Caddy (CT 107).** Build the candidate on the Mac, back up, install, validate, reload:
   ```sh
   # from the repo on the Mac (the script only prints; --only-h3 = PIECE 1 + 3, the #108 fix alone)
   /usr/bin/ssh minix-local 'pct exec 107 -- cat /etc/caddy/Caddyfile' \
     | deploy/ct107/merge_reel_edge.py > /tmp/Caddyfile.reel-edge
   diff <(/usr/bin/ssh minix-local 'pct exec 107 -- cat /etc/caddy/Caddyfile') /tmp/Caddyfile.reel-edge   # review
   /usr/bin/ssh minix-local 'pct exec 107 -- cp -p /etc/caddy/Caddyfile /etc/caddy/Caddyfile.pre-reel-edge-$(date -u +%Y%m%dT%H%M%SZ)'
   /usr/bin/ssh minix-local 'pct exec 107 -- sh -c "cat > /etc/caddy/Caddyfile"' < /tmp/Caddyfile.reel-edge
   ssh minix-local
   # validate needs the Cloudflare token env (the DNS module provisions); sourced, never printed
   pct exec 107 -- sh -c 'set -a; . /etc/caddy/cloudflare.env; set +a; caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile'
   pct exec 107 -- caddy fmt --diff /etc/caddy/Caddyfile      # whitespace only
   pct exec 107 -- systemctl reload caddy
   ```
   Check from the LAN: `curl -sI https://reel.rafikii.co/ | grep -i alt-svc` → `alt-svc: clear`.
   PIECE 1 + 3 alone are the #108 fix and can go in before PIECE 2. Without a public DNS record, PIECE 2
   is unreachable (and the `:8443` catch-all keeps answering 404 for other names). Rollback:
   `cp -p /etc/caddy/Caddyfile.pre-reel-edge-<ts> /etc/caddy/Caddyfile && systemctl reload caddy`.
   If `validate` fails, copy the backup back before anything reloads or restarts Caddy (the running
   process still has the old config).
   **Applied 2026-11-15 (#108 part only):** `--only-h3` output minus PIECE 3's three `header_up X-Forwarded-For /
   X-Real-IP / -Forwarded` lines (kept out to change only h3; Caddy already sets XFF to the peer on `:443`). Backup
   `/etc/caddy/Caddyfile.pre-reel-h3-20261115T034930Z`; `validate` ok, then `reload`. Checked: `alt-svc: clear` on
   reel.rafikii.co, the other 16 LAN sites keep their status codes and `h3=":443"`, immich/ha/plex on `:8443` answer
   without Alt-Svc, no UDP listener on 8443. Rollback:
   `cp -p /etc/caddy/Caddyfile.pre-reel-h3-20261115T034930Z /etc/caddy/Caddyfile && systemctl reload caddy`.
   The live file now has `Alt-Svc "clear"` and a global block, so `merge_reel_edge.py` refuses to run on it: place
   PIECE 2 (and, if wanted, the three `header_up` lines) by hand. Without the env, `validate` fails with `provision tls: … loading TLS automation` (seen 2026-11-15 on the
   current file); that is the missing token, not the config.
   **PIECE 2 applied 2026-11-15** by hand, before the `:8443` catch-all, with the `@lan_only_writes` block (#127).
   Backup `/etc/caddy/Caddyfile.pre-reel-public-20261115T094851Z`. Gotcha: `caddy validate` run as root creates
   `/var/lib/caddy/logs/reel-public-access.json` owned by root, and the reload then fails with `permission denied`
   (the old config keeps running); `chown caddy:caddy` that file and reload. Checked against `10.10.10.73:8443`:
   health, `GET /setup`, `/` → 200; `POST /setup`, `/admin/*` (any case), `/webhooks/*`, `/cast/*`,
   `POST /libraries[/…]`, `POST /requests/*/approve` → 403; HSTS, report-only CSP, `alt-svc: clear`, HTTP/2;
   immich/ha/plex and the LAN site unchanged. Rollback:
   `cp -p /etc/caddy/Caddyfile.pre-reel-public-20261115T094851Z /etc/caddy/Caddyfile && systemctl reload caddy`.
3. **Go live = #32, not part of #31.** Cloudflare DNS: `A reel → 85.144.240.8`, **DNS-only**, TTL auto, no
   AAAA. The first public request makes Caddy issue the cert by DNS-01 (as for immich). Then run
   `deploy/ct107/verify_edge.sh` from outside (Tailscale off). Rollback: delete the record (LAN and
   Tailscale are unaffected; they resolve through Technitium).
4. **After about a week of clean report-only CSP** (DevTools → Console shows `[Report Only]` lines): rename
   `Content-Security-Policy-Report-Only` to `Content-Security-Policy` in PIECE 2, validate, reload.
   Rollback: rename back.

**Caddy unit and the Cloudflare token.** The packaged unit ran `caddy run --environ`, which prints the whole
environment, `CF_API_TOKEN` included, to the journal at every start. Since 2026-11-15 the drop-in
`/etc/systemd/system/caddy.service.d/no-environ.conf` resets `ExecStart` to `/usr/bin/caddy run --config
/etc/caddy/Caddyfile`; Caddy was restarted once and that start logged no `CF_API_TOKEN` (rollback: delete the
drop-in, `systemctl daemon-reload`). **Token rotation is pending** (owner deferred it):
1. Cloudflare → API Tokens → new token, Zone → Zone → Read + Zone → DNS → Edit, zone rafikii.co (the current
   token's scope).
2. Back up `/etc/caddy/cloudflare.env` (`cp -p`), write the new value (root, 0600), `systemctl restart caddy` at a
   quiet moment (Reel `GET /api/v1/admin/sessions` and `/admin/iptv/hub` empty), and check the start log has no
   `tls`/`dns` errors. A quick DNS-01 check: `curl -H "Authorization: Bearer $CF_API_TOKEN"
   https://api.cloudflare.com/client/v4/user/tokens/verify` from CT 107 with the env sourced.
3. Revoke the old token in Cloudflare.
4. Purge the old journal entries: `journalctl --rotate`, then `journalctl --vacuum-time=<time since the restart>`
   (vacuum works per journal file for the whole system journal, not per unit, so say what it removes).
   `journalctl -u caddy -o cat | grep -c CF_API_TOKEN` (name only, never print matches) should then be 0.

**OPNsense: nothing to change for DNS-only.** The WAN `TCP 443 → 10.10.10.73:8443` DNAT and its pass
rule exist (immich/plex use them). Note: that pass rule's description says "Cloudflare_IPv4 only", but the
loaded pf rule allows **any** source (checked 2026-11-15 with `pfctl -s rules`). That's right for the
DNS-only names, but the description is misleading, and `ha.rafikii.co` (proxied) is reachable directly
too. Owner's call.

**Optional, owner's choice:**
- *Geo allowlist (NL + EG).* With DNS-only it can only be done at OPNsense, for the whole public listener
  (immich, ha and plex share the WAN 443 rule): a GeoIP alias (Firewall → Aliases → GeoIP settings,
  needs a MaxMind GeoLite2 licence URL) as the source of the WAN 443 pass rule, plus the
  `Cloudflare_IPv4` alias for ha. Rollback: source back to any. Mobile data abroad often exits in the home
  country (NL) when roaming; a local SIM shows the local country. CrowdSec's blocklists already apply.
- *Caddy rate limit:* `caddy add-package github.com/mholt/caddy-ratelimit` on CT 107, then uncomment the
  block in PIECE 2 and add `order rate_limit before basic_auth` to the global block. Rollback: comment out
  and `caddy remove-package`.

### If the owner picks Cloudflare-proxied (orange cloud) instead

- Use PIECE 1b instead of PIECE 1 (`trusted_proxies` = Cloudflare ranges, `client_ip_headers
  CF-Connecting-IP`, `trusted_proxies_strict`). Caddy then reads the client from Cloudflare only when the
  TCP peer is a Cloudflare address, so a direct connection can't forge it.
- Cloudflare: a Cache Rule "Bypass cache" for `reel.rafikii.co` (by default Cloudflare caches `.mp4`/`.mkv`
  and similar extensions); Network → HTTP/3 is a zone setting (it also covers ha); WebSockets are on by
  default (100 s idle; Reel pings every 25 s); the 100 MB request body limit doesn't matter (Reel takes
  4 MiB). Geo: a WAF custom rule `(http.host eq "reel.rafikii.co" and not ip.src.country in {"NL" "EG"})`
  → Block, per host. Login rate limiting: a rate-limiting rule on `/api/v1/auth/login`.
- OPNsense can **not** restrict WAN 443 to Cloudflare's ranges while immich and plex stay DNS-only on the
  same port.

### Cloudflare Tunnel + Access (the alternative the owner was told about)

No open port and a Cloudflare login in front of Reel. But: Access breaks every client that can't do the
Access login flow, which is the TV apps used away from home, external players opening signed media URLs,
and Chromecast receivers. Video through the tunnel falls under the same Cloudflare terms as the proxy. And
`cloudflared` connects from the LAN: if it pointed at Caddy's `:443` block, every internet client would
look like a LAN address to Reel (LAN-only setup, shared lockout bucket). It would need its own Caddy site
with `trusted_proxies <cloudflared IP>` + `client_ip_headers CF-Connecting-IP` and the same path blocks.

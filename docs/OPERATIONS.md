# Operations

> **No secrets in docs, shells, or chat.** Credentials live in root-only files on the hosts listed below.
> Use hidden prompts (`read -rs`) to set them.

## Topology

| Piece | Where | Notes |
|---|---|---|
| Reel container | Proxmox `minix` → **CT 113** (`plex`, 10.10.10.207) | `/root/reel/docker-compose.yml` + `.env` (root-only). Shares the iGPU (`/dev/dri`) and the read-only media mount with Plex |
| Data | CT 113 `/opt/appdata/reel` → container `/appdata` | `reel.db` (SQLite WAL), `signing.key` (0600), image/transcode caches |
| Media | `/mnt/media/media` → container `/data` (ro) | Same paths as Radarr/Sonarr (`/data/media/...`), so webhooks need no path mapping |
| TLS / name | Caddy **CT 107** (10.10.10.73), LAN block `*.rafikii.co` → `@reel host reel.rafikii.co` → `10.10.10.207:8420` (`flush_interval -1`) | LAN only. Backups: `/etc/caddy/Caddyfile.pre-reel-*` |
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
  `cargo nextest run --workspace` (parallel across test binaries; no doctests exist), debug info is off
  (a CARGO_HOME config written by the job) and linking uses mold. The Rust cache (`shared-key: reel`) is
  saved only from `main`; PRs restore it read-only. Locally, plain `cargo test --workspace` still works.
- `image.yml` keeps its Docker layer cache in the registry (`ghcr.io/mrafik92/reel:buildcache`), not the
  Actions cache, so it can't evict the Rust cache (10 GB repo limit).
- Not done yet (#73): caching generated test media, caching the ffmpeg apt install, sccache.
- Server changes take ~10 min to build; web-only changes take ~1 min (cached Rust layer).
- `image.yml` cancels in-flight builds on new pushes, so wait for the **latest** run.
- Deploy (from the Mac):
  ```sh
  ssh minix-local 'pct exec 113 -- sh -c "cd /root/reel && docker compose pull -q && docker compose up -d && docker image prune -f"'
  curl -s https://reel.rafikii.co/api/v1/health   # {"status":"ok"} (version only when signed in)
  ```
- **Preferred: GitHub.** After an image build, its run summary links to **Actions → deploy → Run workflow**. That click is the gate (web or the GitHub mobile app). The job runs `deploy/ct113/deploy.py --forced` on CT 116: it deploys, health-checks, moves tickets to Deployed and writes a run summary. Runs are serialized, and the job refuses to deploy while a newer image build is still running. Recovery: run it with **since = <old sha>** to (re)process tickets without deploying.
- **Fallback from the Mac:** `deploy/ct113/deploy.py` (or `--dry-run`). It pulls the latest successful image, restarts, runs a health check, then finds tickets referenced in the commits since the previously deployed build. Each gets a comment "🚀 Deployed to CT 113 — build `abc1234`" and moves to **Deployed** on the project board.
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
| `REEL_TRUST_PROXY_HEADERS` | `true` | Trust XFF from private-range peers (Caddy) |
| `REEL_MAX_TRANSCODES` / `REEL_MAX_USER_TRANSCODES` | `3` / `2` | Transcode caps, shared by playback and live TV: a live channel whose video is re-encoded counts once per viewer for the user and once per channel server-wide |
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
   A console line `[player] stalled at … s: segment N requested … ago and still loading` means a request hung
   in flight. Over h3 on Tailscale that's a wedged QUIC connection (#108): try Chrome with `--disable-quic`.
   Disabling h3 for reel.rafikii.co at Caddy (CT 107) would rule it out, but that's an owner decision.
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

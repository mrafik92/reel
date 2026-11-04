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
| Arr stack | **CT 105** (10.10.10.122): Radarr :7878, Sonarr :8989, Sonarr Anime :8990, qBittorrent :8200 (via gluetun) | Each Arr has a "Reel" webhook connection |
| Firewall | CT 113 Proxmox firewall drops LAN egress apart from the Arr ports and **qBittorrent :8200** (added for Reel) | Backup: `/root/113.fw.bak-*` |
| Remote access | Tailscale; OPNsense has a static-port outbound NAT + WAN UDP 41641 pass (Firewall → Automation, "Tailscale: …") | Config backup `/conf/backup/config-pre-tailscale-static-port-20261103T165802Z.xml` |
| Image registry | `ghcr.io/mrafik92/reel` (private) | CT 113 is logged in with a **pull-only** PAT (`read:packages`) |

### Credentials and where they live

| Secret | Location |
|---|---|
| Reel admin (`rafik`) | minix `/root/.reel-admin` (root-only). Change the password in Settings |
| TMDB key, webhook secret, (OMDb, Trakt) | CT 113 `/root/reel/.env` |
| IPTV provider login, qBittorrent password, Arr API keys | Reel DB (entered in Settings) |
| OPNsense API key (firewall automation) | Mac `~/.config/opnsense-api` (0600). Revoke in OPNsense if no longer needed |

## Build & deploy pipeline

```
git push main ──► GitHub Actions
                   ├── ci.yml:    cargo fmt/clippy/test (rustc 1.86, ffmpeg installed), bun typecheck/build
                   └── image.yml: docker buildx (deploy/Dockerfile) → ghcr.io/mrafik92/reel:latest + :sha-xxxx
                                  build args GIT_SHA / BUILD_DATE → shown in Settings → About
CT 113:  docker compose pull && docker compose up -d && docker image prune -f
```

- Server changes take ~10 min to build; web-only changes take ~1 min (cached Rust layer).
- `image.yml` cancels in-flight builds on new pushes, so wait for the **latest** run.
- Deploy (from the Mac):
  ```sh
  ssh minix-local 'pct exec 113 -- sh -c "cd /root/reel && docker compose pull -q && docker compose up -d && docker image prune -f"'
  curl -s https://reel.rafikii.co/api/v1/health   # {"status":"ok"} (version only when signed in)
  ```
- **Preferred:** `deploy/ct113/deploy.py` (or `--dry-run`). It pulls the latest successful image, restarts, runs a health check, then finds tickets referenced in the commits since the previously deployed build. Each gets a comment "🚀 Deployed to CT 113 — build `abc1234`" and moves to **Deployed** on the project board.
- Rollback: pin `image: ghcr.io/mrafik92/reel:sha-<previous>` in the compose file and run `up -d`.

### Ticket flow (GitHub Project #1)

Backlog → To do → Implementing → Reviewing → **Deployed** (automatic, `deploy.py`) → Verified (manual, fill *Verified on*) → Done (close the issue).

- Reference tickets in commits with `Refs #12`. Use `Fixes #12` only when no real-setup verification is needed; GitHub closes it on push, skipping Deployed/Verified.
- Built-in project workflows (Project → ⋯ → Workflows): *Item added → Backlog*, *Item closed → Done*, *Auto-close issue (Status = Done)*, *Pull request linked to issue → Reviewing*, *Auto-add to project* (`is:issue,pr is:open` on `mrafik92/reel`).

## Configuration (CT 113 compose → env)

| Var | Value | Purpose |
|---|---|---|
| `REEL_DATA_DIR` | `/appdata` | DB, caches, signing key |
| `REEL_PUBLIC_URL` | `https://reel.rafikii.co` | Invite links; never derive URLs from Host |
| `REEL_CAST_BASE_URL` | `http://10.10.10.207:8420` | Absolute LAN URLs for TVs/Chromecast |
| `REEL_TRUST_PROXY_HEADERS` | `true` | Trust XFF from private-range peers (Caddy) |
| `REEL_MAX_TRANSCODES` / `REEL_MAX_USER_TRANSCODES` | `3` / `2` | Transcode caps |
| `REEL_ALLOW_PRIVATE_FETCH` / `..._LOGO_FETCH` | `true` / `false` | SSRF policy (playlists/EPG may be LAN; logos never) |
| `REEL_TMDB_API_KEY`, `REEL_WEBHOOK_SECRET` | in `.env` | |
| `REEL_OMDB_API_KEY`, `REEL_TRAKT_CLIENT_ID` | optional | IMDb/RT scores, Trakt comments |
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

**IPTV provider blocks (HTTP 46x)**
The panel's anti-abuse limit. Don't hammer it; Reel backs off automatically. Check the source status in Settings → Live TV.
A single-connection plan means only one stream at a time; your own newest stream wins.

**TV guide**
Settings → Live TV → Providers → Guides: ordered list, per-guide stats, "Suggested guides" (epgshare01). Use
**Refresh guide** after changes. A per-channel EPG override is in the channel manager.

**qBittorrent**
URL must be `http://10.10.10.122:8200` (the CT 113 firewall allows only that). qBittorrent 5 cookies (`QBT_SID_*`) are supported.

**Adding a DLNA TV** (pending)
1. Get the TV's IP and model.
2. CT 113 firewall: allow TCP out to the TV's UPnP port (Samsung 9197; LG somewhere in 1000–2000), plus TCP in from the TV to 8420.
3. `POST /api/v1/admin/cast/devices {"ip": "<tv>"}`, or use Settings.

**Bootstrap a fresh instance**
`deploy/ct113/bootstrap.py` (on minix) creates the admin, libraries, Arr servers and webhooks. It's idempotent and never prints keys.

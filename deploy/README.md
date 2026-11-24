# Deploying Reel

> UI locations below ("Admin → …") are indicative; every setting is also available through the REST API
> (`/api/v1/admin/...`, see `server/openapi.json`).

Docker is the supported way to run Reel: one image contains the server, the web client and ffmpeg
(with VAAPI / QSV / NVENC support). Everything below uses the files in this directory.

| File | Purpose |
|---|---|
| `Dockerfile` | multi-stage build: bun/vite web client → cargo server → Debian runtime + ffmpeg (non-root, healthcheck) |
| `docker-compose.yml` | the Reel service (+ optional Caddy via the `caddy` profile) |
| `docker-compose.nogpu.yml` | override for hosts without `/dev/dri` |
| `Caddyfile` | HTTPS reverse proxy with streaming-friendly settings |
| `.env.example` | all variables the compose file reads |
| `config/config.toml.example` | optional config file (library bootstrap, limits) |

## 1. Install

```sh
git clone https://github.com/mrafik92/reel.git && cd reel/deploy
cp .env.example .env            # set MEDIA_DIR, REEL_WEBHOOK_SECRET, RENDER_GID/VIDEO_GID …
cp config/config.toml.example config/config.toml    # optional: pre-create libraries
docker compose up -d --build    # first build takes a few minutes
docker compose logs -f reel
```

Open `http://<host>:8420`. **The first visit creates the admin account** (`GET /api/v1/setup` says
`needs_setup: true` until then; afterwards setup is locked). Then add libraries in the admin UI
(paths are *inside* the container: `MEDIA_DIR` is mounted read-only at `/media`, so use
`/media/movies`, `/media/tv`, …) — or pre-create them in `config/config.toml`.

State lives in the `reel-data` volume (`/data`): SQLite database, image cache, transcode scratch and
`signing.key` (HMAC key for signed stream URLs). Back that volume up. The container runs as UID 1000;
if you switch `reel-data` to a bind mount, `chown -R 1000:1000` it. Reel keeps that directory `0700` and the
database and key `0600` (it tightens them at every start), so a backup job must run as UID 1000 or root.

Updating: `git pull && docker compose up -d --build`. Migrations run automatically at start.

Multi-arch image: `docker buildx build --platform linux/amd64,linux/arm64 -f deploy/Dockerfile -t reel ..`
(Intel iHD/i965 drivers are only installed on amd64; Mesa VAAPI for AMD is on both).

## 2. Friends and family: invites

Admin → Users → **Invite**: choose which libraries the new user may see and optional limits
(concurrent streams, transcodes, bitrate cap, request quota, auto-approve). You get a single-use link
`<REEL_PUBLIC_URL>/invite/<code>` (expires after 7 days by default). The recipient picks a username and
password and is signed in. Users only ever see the libraries they were granted (everything else is a
404); admins can change access, limits, role, disable or delete accounts at any time (disabling signs the
user out everywhere).

Set `REEL_PUBLIC_URL` to the address friends will use, otherwise links are built from the request's
`Host` header.

## 3. Radarr / Sonarr: requests and instant availability

1. Admin → Settings → Radarr/Sonarr → add each server (URL + API key). Reel validates the key and
   offers its quality profiles and root folders. Set a **default profile and root folder**.
2. Put a long random string in `REEL_WEBHOOK_SECRET` (e.g. `openssl rand -hex 24`).
3. In Radarr/Sonarr: *Settings → Connect → + → Webhook*
   * URL: `http://reel:8420/api/v1/webhooks/arr?server=<ID>&token=<REEL_WEBHOOK_SECRET>`
     (`<ID>` is the server id shown in Reel; use `http://<host>:8420/...` if Radarr runs outside the
     compose network)
   * Method `POST`, triggers: **On Grab, On Import, On Upgrade, On Movie/Episode File Delete**.
   * Click *Test* — Reel answers 200 for `Test` events.
4. Webhooks are rejected (403) while the secret is empty. Imported files are scanned immediately
   (targeted rescan) and the request flips to *available* without waiting for a full scan.

The Radarr/Sonarr **root folders must map to paths Reel can see**: if Radarr imports to `/movies`,
mount the same host directory into Reel and use that path in the library, or mount it at the same
container path in both.

Optional: add qBittorrent/Transmission under *Download clients* for speed/ETA/peers on the progress ring.

## 4. Remote access: port forwarding + Caddy

1. DNS: create an `A`/`AAAA` record (e.g. `reel.example.com`) for your public IP (use a DDNS service on
   a dynamic IP).
2. Router: forward **TCP 80 and 443** (and **UDP 443** for HTTP/3) to the Docker host.
3. `.env`:
   ```
   REEL_DOMAIN=reel.example.com
   ACME_EMAIL=you@example.com
   REEL_PUBLIC_URL=https://reel.example.com
   REEL_BIND_HOST=127.0.0.1          # Reel itself is then reachable only through Caddy
   REEL_TRUSTED_PROXIES=172.16.0.0/12  # the Caddy container's network: per-IP login limits and the
                                       # LAN-only setup check see the real client IP
   ```
4. `docker compose --profile caddy up -d` — Caddy obtains and renews the certificate automatically.

The `Caddyfile` is tuned for media: no compression of video, `flush_interval -1` so live TV and HLS
stream immediately, WebSocket (`/events`) works out of the box, no read/write timeout on responses
(a movie is one long response) and a 2-minute response-header timeout for cold transcode starts.

> Use a proper reverse proxy or a VPN (WireGuard/Tailscale). Cloudflare's free proxy/tunnel forbids
> serving video through it.

Security notes: passwords are Argon2id; session tokens are opaque and stored hashed; logins are
rate-limited per user and per IP with exponential lock-out; stream/image/subtitle URLs are signed and
expire; the audit log (Admin → Audit) records admin actions and login failures.

**Client IP behind a proxy.** Set `REEL_TRUSTED_PROXIES` to your proxy's address or network
(comma-separated, e.g. `10.0.0.5` or `172.16.0.0/12,fd00::/8`). Reel believes `X-Forwarded-For` only from
those peers and takes the right-most entry that isn't one of them. Make the proxy **overwrite** the header
(Caddy: `header_up X-Forwarded-For {client_ip}`), never pass on what the client sent. The older
`REEL_TRUST_PROXY_HEADERS=true` trusts every private-range peer, so any machine on your LAN could choose
its IP; it still works but logs a warning.

**Exposing Reel to the internet:** keep admin, first-run setup, webhooks and casting off the public
address. `deploy/ct107/reel.caddy` is a worked example (public site with path blocks, CSP,
Permissions-Policy, HSTS, no HTTP/3), and `deploy/ct107/verify_edge.sh` checks it from outside.

## 5. Hardware transcoding

Direct play and remux need no GPU. Transcoding (and IPTV re-encodes of non-H.264 sources) uses the
best available encoder, detected at startup with a 1-frame test encode per backend, falling back to
software (libx264) automatically — also at runtime if a hardware encode fails.

**Intel / AMD (VAAPI, QSV)** — the default compose file passes `/dev/dri` through and adds the host's
`render` and `video` groups so the non-root user can open `renderD128`:

```sh
getent group render | cut -d: -f3     # → RENDER_GID in .env
getent group video  | cut -d: -f3     # → VIDEO_GID
ls -l /dev/dri                        # renderD128 should exist on the host
```

**Check it works:** sign in as admin and open `GET /api/v1/admin/hwaccel` (Admin → System):

```json
{ "selected": {"type": "vaapi", "device": "/dev/dri/renderD128"},
  "available": [{"type":"vaapi", …}], "notes": [] }
```

On Intel, `selected.low_power` lists the low-power (fixed-function) encoders that passed their own test
encode (`{"h264": true, "hevc": true}` on an N100); VOD transcodes use them with `-low_power 1`. If one
fails at runtime the session retries with the regular encoder and `degraded` shows `vaapi-lp`. Set
`REEL_VAAPI_LOW_POWER=0` to keep the regular encoders.

`selected.type == "software"` means no hardware path passed the test; `notes` says why (missing device,
permission denied → check the group ids, driver init failed …). From inside the container you can also
run `docker compose exec reel vainfo`. IPTV transcode status is under `GET /api/v1/admin/iptv/hub`
(`transcode.encoder`, `fell_back_to_software`).

No GPU / macOS / Windows Docker Desktop: add the override so compose does not look for `/dev/dri`:
`docker compose -f docker-compose.yml -f docker-compose.nogpu.yml up -d`.

**NVENC (NVIDIA):** install the NVIDIA Container Toolkit, then add to the `reel` service:
```yaml
    runtime: nvidia
    environment:
      NVIDIA_VISIBLE_DEVICES: all
      NVIDIA_DRIVER_CAPABILITIES: compute,video,utility
```
Debian's ffmpeg loads the NVENC libraries at runtime, so no image change is needed.

Limit concurrent transcodes with `REEL_MAX_TRANSCODES` / `REEL_MAX_USER_TRANSCODES` (or per user in
the admin UI). Browsers that switch quality themselves get a multi-variant stream: the original plus
lower-quality versions that are only transcoded while they're being watched, each counting against those
limits while it runs. `REEL_ABR=0` turns that off (one version per stream); `REEL_ABR_VARIANT_IDLE_SECS`
(default 30) sets how soon an unwatched version stops.

## 6. IPTV

Admin → Live TV → **Add source**:

* **M3U / M3U8** — playlist URL (`http(s)://…`). Channel attributes `tvg-id`, `tvg-logo`,
  `group-title`, `tvg-chno`, catch-up info, and per-channel user-agent/referrer are honoured; an
  `url-tvg` header in the playlist is used as the guide URL when you don't set one.
* **Xtream Codes** — server base URL + username + password. Channels, categories, logos, stream URLs
  and the XMLTV guide come from the provider; **Test connection** shows account status, expiry and the
  allowed/active connection count, and `max_connections` is filled in from it.
* Set **max connections** to your provider's limit (Reel then never opens more upstream connections
  than that — see below). Optionally set a user-agent the provider requires, and an EPG URL (XMLTV,
  `.xml` or `.gz`).

Sources refresh on a schedule (every 12 h by default; per-source interval, `0` = manual) and with
**Refresh now**. Refreshes keep channel ids, favorites and your edits (renames, numbers, disabled
channels, manual EPG mapping); channels that vanish from the provider are hidden, not deleted. The
same `tvg-id` on two sources automatically becomes a **failover alias**.

How it stays fast and within your provider limit: one upstream connection per channel no matter how
many viewers, a GOP cache so a zap starts at a keyframe immediately, a grace period
(`iptv_grace_secs`, 15 s) so channel-surfing back is instant, warm "previous channel" connections that
are evicted first when a slot is needed, and — when every slot is actively watched — a clear
`429 connection limit … in use by: <channels>` instead of kicking a viewer. `iptv_warm_favorites = N`
keeps the N most-favorited channels pre-connected when the budget allows. Live status: Admin → Live TV →
*Hub* (`/api/v1/admin/iptv/hub`: bitrate, time-to-first-byte, reconnects, failovers, TS errors).

Native players (Tizen, webOS, Android) should tune with `"mode": "passthrough"` (raw MPEG-TS, zero
CPU); browsers use `"hls"` (video copied, audio re-encoded to AAC only when needed).

### Server-side fetch policy (SSRF)

Reel fetches admin-supplied URLs on the server: playlists, EPG, Xtream APIs, channel streams and logos.

| Setting | Default | Covers |
|---|---|---|
| `REEL_ALLOW_PRIVATE_FETCH` | `true` | playlists, EPG, Xtream API, stream upstreams — these are admin-trusted and often LAN-hosted (tvheadend, HDHomeRun, a NAS) |
| `REEL_ALLOW_PRIVATE_LOGO_FETCH` | `false` | channel logos — they come from third-party playlists |

With a flag off, requests to loopback, private (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7),
link-local (incl. cloud metadata `169.254.169.254`), multicast and unspecified addresses are refused —
for literal IPs, for every address a hostname resolves to (DNS-rebinding safe) and for every redirect
hop; the admin API then rejects such source URLs with an explanation. Only `http://` and `https://`
are accepted. Limitation: for HLS-style providers (`.m3u8`), ffmpeg fetches the segments itself; only
the playlist URL is checked, but ffmpeg is restricted to network protocols (no `file:`/`concat:`).

## 6b. Play on TV (DLNA)

Reel finds UPnP/DLNA MediaRenderers (LG webOS, Samsung Tizen, Sony, ...) by SSDP every minute, or you
add a TV by IP (`POST /api/v1/admin/cast/devices {"ip":"192.168.1.40"}`, probes Samsung `:9197/dmr`,
LG `:1x99/`, ... and is remembered in `/data/cast_devices.json`).

1. **Compose:** SSDP needs host networking: `docker compose -f docker-compose.yml -f docker-compose.host.yml up -d`.
   (Without it, add the TVs by IP: that needs no multicast.)
2. **`.env`:** `REEL_CAST_BASE_URL=http://<LAN ip of the Reel host>:8420`. The TV fetches the media from this
   address, so it must be reachable from the TV (not a name the TV cannot resolve).
3. **Firewall (Proxmox CT / host firewall that drops LAN egress except allowed IPs).** Allow from the Reel
   host to each TV (`<tv ip>`):
   - UDP 1900 out to multicast `239.255.255.250` (SSDP search) and UDP in from the TV (replies come from
     the TV to a random high UDP port: allow established/related, or UDP from `<tv ip>`);
   - TCP out to the TV's UPnP ports for the description and SOAP control: Samsung `9197` (also `7676`, `55000`
     on some models), LG `1x99` (observed: `1990` / `1199`-`1999`; check with `nmap -p 1000-2000 <tv ip>`),
     Sony `52323`. Open the one your TV uses (shown by the `location` of its SSDP answer);
   - TCP **in** from the TV to Reel `8420` (the TV downloads the video, subtitles and HLS playlists from Reel).
4. Per-TV quirks: Samsung gets the subtitle as `sec:CaptionInfoEx` (WebVTT). LG webOS ignores sidecar subtitles
   over DLNA. Live channels are sent as raw MPEG-TS; library/VOD files direct when the TV can play the
   container/codecs, otherwise as an HLS playlist.

## 7. Troubleshooting

* `docker compose logs reel` — `RUST_LOG=debug` for more.
* `curl http://localhost:8420/api/v1/health` → `{"status":"ok",…}`; the container healthcheck uses the same.
* Library empty: the container user (UID 1000) must be able to read `MEDIA_DIR`.
* Moved or renamed files keep their watch history (same size/duration/streams are matched), also when
  the rename arrives as two file-watcher events a few seconds apart.
* Behind a proxy all users share one IP in the audit log / rate limiter → set
  `REEL_TRUSTED_PROXIES` to the proxy's address (see §4).

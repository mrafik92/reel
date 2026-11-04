# Reel documentation

Reel is a self-hosted media server, a Plex/Jellyfin alternative. It brings together your local library, Seerr-style
requests to Radarr/Sonarr with live download progress, and an IPTV engine (live TV, guide, and the provider's
movies and series), with clients for the web, LG webOS and Samsung Tizen. It runs as a single Rust binary
with a SolidJS web app and needs no cloud account.

| Document | What it covers |
|---|---|
| [FEATURES.md](FEATURES.md) | Every feature, what it does, and where it lives |
| [ARCHITECTURE.md](ARCHITECTURE.md) | How the pieces fit together, data flows, and the design choices with their reasons |
| [STATUS.md](STATUS.md) | Current state as of 2026-11-04: what works, what's verified, known gaps, security posture, backlog |
| [OPERATIONS.md](OPERATIONS.md) | The live deployment (Proxmox CT 113), build/deploy pipeline, config, runbooks |
| [../SPEC.md](../SPEC.md) | The original design spec (v0.1). Historical; these docs supersede it where they differ |
| [../deploy/README.md](../deploy/README.md) | Generic install guide (Docker Compose, Caddy, GPU, IPTV, DLNA) |

Quick orientation:

```
reel/
├── server/      Rust workspace (axum, sqlx/SQLite, ffmpeg subprocesses). Binary: reel-server
├── clients/     bun workspace: apps/web (SolidJS), apps/webos, apps/tizen, packages/ui, packages/sdk
├── deploy/      Dockerfile, compose files, Caddyfile, CT 113 helper scripts
├── docs/        you are here
└── .github/     CI (fmt, clippy, tests, typecheck, build) + image build → ghcr.io/mrafik92/reel
```

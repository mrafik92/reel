# Reel

Self-hosted media server: library + playback, built-in requests (search → Add → live download progress),
and a low-latency IPTV engine. No cloud account. Documentation: **[docs/](docs/README.md)** — [features](docs/FEATURES.md), [architecture & design choices](docs/ARCHITECTURE.md), [current status](docs/STATUS.md), [operations](docs/OPERATIONS.md). Original design: [SPEC.md](SPEC.md).

## Layout

```
server/    Rust workspace (axum, sqlx/SQLite, ffmpeg subprocesses)
clients/   bun workspace: @reel/sdk (generated), @reel/ui (SolidJS), apps/web, apps/webos, apps/tizen
```

## Develop

```sh
# server (http://localhost:8420)
cd server && cargo run -p reel-api --bin reel-server

# regenerate OpenAPI + TS SDK after API changes
cd server && cargo run -q -p reel-api --bin export-openapi > openapi.json
cd clients && bun run --filter @reel/sdk generate

# web client (proxies /api to :8420)
cd clients && bun install && bun run dev
```

## Status

Live on the homelab (CT 113). See [docs/STATUS.md](docs/STATUS.md).

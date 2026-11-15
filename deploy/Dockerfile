# syntax=docker/dockerfile:1.7
#
# Reel server + web client in one image.
#   build:  docker build -f deploy/Dockerfile -t reel .        (context = repo root)
#   multi-arch: docker buildx build --platform linux/amd64,linux/arm64 -f deploy/Dockerfile -t reel .
#
# Stages: web (bun/vite) → build (cargo) → runtime (debian + ffmpeg with VAAPI/QSV/NVENC support).

ARG RUST_VERSION=1.86
ARG BUN_VERSION=1
# Build metadata (CI passes these; shown in Settings → About).
ARG GIT_SHA=""
ARG BUILD_DATE=""

# ── 1. web client (platform independent → always built on the build host's arch) ──────────────
FROM --platform=$BUILDPLATFORM oven/bun:${BUN_VERSION} AS web
WORKDIR /clients
COPY clients/package.json clients/bun.lock clients/tsconfig.base.json ./
COPY clients/packages ./packages
COPY clients/apps ./apps
RUN --mount=type=cache,target=/root/.bun/install/cache bun install --frozen-lockfile
ARG GIT_SHA
ARG BUILD_DATE
RUN cd apps/web && VITE_GIT_SHA="$GIT_SHA" VITE_BUILD_DATE="$BUILD_DATE" bun run build

# ── 2. server ───────────────────────────────────────────────────────────────────────────────────
# cargo-chef: the dependencies are built in their own layer, keyed only by Cargo.toml/Cargo.lock (the
# "recipe"), so CI's registry layer cache reuses them until a dependency changes. BuildKit cache
# mounts can't do that on CI: they live on the builder, and every GitHub runner starts empty.
FROM lukemathwalker/cargo-chef:0.1.71-rust-${RUST_VERSION}-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY server/ ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p reel-api --bin reel-server --recipe-path recipe.json
COPY server/ ./
RUN cargo build --release --locked -p reel-api --bin reel-server \
    && strip target/release/reel-server \
    && cp target/release/reel-server /reel-server

# ── 3. runtime ──────────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
ARG TARGETARCH

# ffmpeg (Debian's build includes VAAPI, QSV, NVENC/NVDEC loaders, libx264/x265, libvpx …) plus the
# userspace drivers: Intel iHD (Gen8+, non-free) and i965 (older) on amd64, Mesa VA for AMD.
RUN set -eux; \
    sed -i 's/^Components: main$/Components: main non-free non-free-firmware/' /etc/apt/sources.list.d/debian.sources; \
    apt-get update; \
    pkgs="ffmpeg ca-certificates curl tzdata libva-drm2 vainfo mesa-va-drivers"; \
    if [ "$TARGETARCH" = "amd64" ]; then \
        pkgs="$pkgs intel-media-va-driver-non-free i965-va-driver"; \
    fi; \
    apt-get install -y --no-install-recommends $pkgs; \
    rm -rf /var/lib/apt/lists/*

# Non-root. GPU access comes from `group_add` (render/video gids of the host) in compose.
RUN useradd --system --uid 1000 --user-group --create-home --home-dir /home/reel --shell /usr/sbin/nologin reel \
    && usermod -aG video reel \
    && mkdir -p /data /config /app/web \
    && chown -R reel:reel /data /config

COPY --from=build /reel-server /usr/local/bin/reel-server
COPY --from=web /clients/apps/web/dist /app/web

ENV REEL_BIND=0.0.0.0:8420 \
    REEL_DATA_DIR=/data \
    REEL_WEB_DIR=/app/web \
    REEL_CONFIG=/config/config.toml \
    RUST_LOG=info,tower_http=warn

ARG GIT_SHA
ARG BUILD_DATE
ENV REEL_GIT_SHA=$GIT_SHA \
    REEL_BUILD_DATE=$BUILD_DATE

USER reel
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8420

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8420/api/v1/health || exit 1

STOPSIGNAL SIGTERM
ENTRYPOINT ["reel-server"]

LABEL org.opencontainers.image.title="Reel" \
      org.opencontainers.image.description="Self-hosted media server: library, requests, IPTV" \
      org.opencontainers.image.licenses="MIT"

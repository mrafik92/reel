FROM rust:1.86-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p reel-api --bin reel-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/reel-server /usr/local/bin/reel-server
ENV REEL_BIND=0.0.0.0:8420 REEL_DATA_DIR=/data
VOLUME /data
EXPOSE 8420
ENTRYPOINT ["reel-server"]

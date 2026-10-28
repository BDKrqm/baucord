# baucord-gateway, built the way the server's image is (the root
# Dockerfile): the same chef base, the same `deploy` profile, the same
# distroless runtime. The one difference is `cmake`, which the voice
# path's libopus needs to build. Build from the repo root:
#   docker build -f crates/gateway/Dockerfile -t baucord-gateway .
ARG RUST_VERSION=1.95
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION}-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY third_party third_party
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
COPY rust-toolchain.toml ./
COPY third_party third_party
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --profile deploy --package gateway --bin baucord-gateway --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --profile deploy --locked --package gateway --bin baucord-gateway
# An empty, runtime-owned /data for the distroless stage (no shell
# there to make one).
RUN mkdir -p /app/data-empty

FROM gcr.io/distroless/cc-debian12
COPY --from=builder --chown=10000:10000 /app/target/deploy/baucord-gateway /app/baucord-gateway
# The key and the pin live in /data (a volume); the runtime uid owns it.
COPY --from=builder --chown=10000:10000 /app/data-empty /data
WORKDIR /data
USER 10000
ENV BAUCORD_KEY=/data/bot.key
# A container's bot is on another host: the token is mandatory off the
# loopback (README §1), and the port is published to the bot's side.
ENV BAUCORD_GATEWAY_LISTEN=0.0.0.0:8765
EXPOSE 8765/tcp
# Ctrl-C and SIGTERM alike close the session before the process ends.
STOPSIGNAL SIGTERM
ENTRYPOINT ["/app/baucord-gateway"]

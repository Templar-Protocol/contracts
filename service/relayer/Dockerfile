# syntax=docker/dockerfile:1

ARG NODE_VERSION=24.14.0
ARG NODE_DIST=node-v24.14.0-linux-x64
ARG NODE_SHA256=41cd79bb7877c81605a9e68ec4c91547774f46a40c67a17e34d7179ef11729df

FROM debian:bookworm-slim AS node-dist
ARG NODE_VERSION
ARG NODE_DIST
ARG NODE_SHA256

# Fetch Node once, verify it, then reuse it in later stages.
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl xz-utils && \
    rm -rf /var/lib/apt/lists/*

RUN /bin/bash -o pipefail -ec 'curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/${NODE_DIST}.tar.xz" -o /tmp/node.tar.xz && \
    echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - && \
    mkdir -p /opt/node && \
    tar -xJf /tmp/node.tar.xz -C /opt/node --strip-components=1 && \
    rm /tmp/node.tar.xz'

FROM rust:1.86.0 AS rust-builder
COPY --from=node-dist /opt/node /usr/local

WORKDIR /usr/src/relayer
COPY . .

ENV SQLX_OFFLINE=true
# Ensure that the RedStone bridge build step installs dependencies with npm ci
ENV CI=true

# The RedStone bridge build script runs npm during cargo build.
RUN node --version && npm --version
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/rustup \
    cargo build --package templar-relayer --profile release

FROM debian:bookworm-slim AS production

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl libssl3 && \
    rm -rf /var/lib/apt/lists/*

RUN groupadd --system relayer && \
    useradd --system --gid relayer --create-home --home-dir /home/relayer relayer

# Runtime still needs `node` because the relayer spawns the bridge process.
COPY --from=node-dist /opt/node /usr/local
COPY --from=rust-builder /usr/src/relayer/target/release/templar-relayer /usr/local/bin/templar-relayer

WORKDIR /app
ENV NODE_ENV=production
USER relayer

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["sh", "-c", "curl -fsS http://127.0.0.1:${PORT:-3000}/healthz || exit 1"]

CMD ["templar-relayer"]

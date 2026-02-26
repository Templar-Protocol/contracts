# syntax=docker/dockerfile:1

FROM rust:1.86.0 AS rust-builder

WORKDIR /usr/src/relayer

COPY . .

ENV SQLX_OFFLINE=true
RUN \
    # cache dependencies, etc.
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/rustup \
    cargo build --package templar-relayer --profile release

# Node builder
FROM node:24-slim as node-builder
ENV NODE_ENV=development

WORKDIR /usr/src/redstone-bridge

COPY service/relayer/redstone-bridge/package*.json ./
RUN npm ci

COPY service/relayer/redstone-bridge/tsconfig.json ./
COPY service/relayer/redstone-bridge/src ./src
RUN npm run build

# Runtime image
FROM node:24-slim as production
ENV NODE_ENV=production

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app/redstone-bridge/dist
WORKDIR /app/redstone-bridge
COPY service/relayer/redstone-bridge/package*.json .
RUN npm ci

COPY --from=rust-builder /usr/src/relayer/target/release/templar-relayer /usr/local/bin/templar-relayer
COPY --from=node-builder /usr/src/redstone-bridge/dist/* /app/redstone-bridge/dist

WORKDIR /app
ENV REDSTONE_BRIDGE_PATH=/app/redstone-bridge/dist/index.js

CMD [ "templar-relayer" ]

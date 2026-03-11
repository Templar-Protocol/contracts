# syntax=docker/dockerfile:1.7
ARG RUST_VERSION=1.86
ARG DEBIAN_VERSION=bookworm
ARG CHEF_VERSION=0.1.71

# Setup stage
FROM lukemathwalker/cargo-chef:${CHEF_VERSION}-rust-${RUST_VERSION}-slim-${DEBIAN_VERSION} AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Prepare stage
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build stage
FROM chef AS builder
ENV SQLX_OFFLINE=true
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --locked --recipe-path recipe.json

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && mkdir -p /app/bin \
    && find target/release -maxdepth 1 -type f -executable -exec cp {} /app/bin/ \;

# Final stage
FROM debian:${DEBIAN_VERSION}-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    procps \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/bin/* /app/bin/

USER 10001:10001

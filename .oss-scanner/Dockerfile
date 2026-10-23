# Image for Anthropic's OSS Scanner (https://red.anthropic.com/oss-scanner/).
#
# The scan runs with no network access. This build fetches everything the tests need, then
# builds the workspace the way CI does (.github/workflows/test.yml):
#   - every crate in Cargo.lock, including git dependencies;
#   - the npm packages that service/redstone-bridge's build script bundles with Node.js;
#   - the released contract Wasms that the migration and upgrade tests deploy;
#   - the neard sandbox binary for the node-backed tests;
#   - every test binary and every workspace binary (dev profile, line tables);
#   - the NEAR contract Wasms in target/near that `just test-sandbox --stale` reuses;
#   - a local Postgres with the role and database CI uses.
# .oss-scanner/threat_model.md says how to run each test gate offline.
#
# BuildKit reads .oss-scanner/Dockerfile.dockerignore instead of the root .dockerignore. The root
# file hides script/, audits/ and docs/, which the test gates and the scanner need.

# Debian trixie ships Postgres 17, the same major version as CI.
FROM rust:1-trixie
ENV DEBIAN_FRONTEND=noninteractive

# libssl-dev and libdbus-1-dev: openssl-sys and libdbus-sys (through keyring).
# clang: the cc builds in secp256k1-sys, zstd-sys and others.
# postgresql: the #[sqlx::test] fixtures in the relayer and the gateway store.
# nodejs and npm: service/redstone-bridge/build.rs runs npm to build and bundle its TypeScript
# sidecar, and the bridge runs that bundle with node. CI gets both from the GitHub runner image.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git jq clang pkg-config libssl-dev libdbus-1-dev postgresql python3 \
      nodejs npm \
    && rm -rf /var/lib/apt/lists/* \
    && node --version && npm --version

# Test tools at pinned versions, checked by SHA-256. The cargo-near x86-64 hash is the one that
# .github/actions/cargo-near pins.
ARG NEXTEST_VERSION=0.9.149
ARG JUST_VERSION=1.59.0
ARG CARGO_NEAR_VERSION=0.19.2
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64) \
        triple=x86_64-unknown-linux-gnu; just_triple=x86_64-unknown-linux-musl; \
        nextest_sha=32d1d8c0567b7a80bee2bad8bb09125c846da96bc009b2de11ec08557290c770; \
        just_sha=409044617b6f06517c1035e55ca38c8c856ff8b650e41bc2aa1ef82d5903a23e; \
        near_sha=4be2fef36808d94fbe224277add6183cb42f395bb2ce272aa35be8cc09bf3206 ;; \
      aarch64) \
        triple=aarch64-unknown-linux-gnu; just_triple=aarch64-unknown-linux-musl; \
        nextest_sha=6f82e375187cf3a1f3e336879a51940c315a5d0ce7253f7e517ad28295b74189; \
        just_sha=23871b1702bfd550fb5f901778fd4290ecf2ee034a45a889fbe96d89b188086d; \
        near_sha=7b8d8ff3da78f8d8b715e9b44acd87fc46b0a29ecfdbe636b38056328837370f ;; \
      *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;; \
    esac; \
    tmp="$(mktemp -d)"; cd "$tmp"; \
    curl -fsSL -o nextest.tgz "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-${triple}.tar.gz"; \
    echo "${nextest_sha}  nextest.tgz" | sha256sum -c -; \
    tar -xzf nextest.tgz -C /usr/local/bin cargo-nextest; \
    curl -fsSL -o just.tgz "https://github.com/casey/just/releases/download/${JUST_VERSION}/just-${JUST_VERSION}-${just_triple}.tar.gz"; \
    echo "${just_sha}  just.tgz" | sha256sum -c -; \
    tar -xzf just.tgz -C /usr/local/bin just; \
    curl -fsSL -o cargo-near.tgz "https://github.com/near/cargo-near/releases/download/cargo-near-v${CARGO_NEAR_VERSION}/cargo-near-${triple}.tar.gz"; \
    echo "${near_sha}  cargo-near.tgz" | sha256sum -c -; \
    tar -xzf cargo-near.tgz --strip-components=1 -C /usr/local/bin "cargo-near-${triple}/cargo-near"; \
    cd /; rm -rf "$tmp"; \
    cargo nextest --version; just --version; cargo near --version

# scanner contract: the checkout lives inside the image, at /src
COPY . /src
WORKDIR /src

# The Cargo settings CI uses. They stay set in the image, so a rebuild during the scan reuses
# the artifacts built here. DATABASE_URL points the test recipes at the local Postgres below,
# so script/postgres-up.sh does not try to start Docker.
ENV CARGO_TERM_COLOR=never \
    CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_DEV_DEBUG=line-tables-only \
    CARGO_PROFILE_TEST_DEBUG=line-tables-only \
    SQLX_OFFLINE=true \
    RUST_BACKTRACE=1 \
    DATABASE_URL=postgres://relayeruser:password@localhost:5432/relayer \
    TEMPLAR_ARTIFACT_CACHE=/opt/templar/artifacts \
    NEAR_SANDBOX_BIN_PATH=/opt/templar/near-sandbox/near-sandbox \
    npm_config_cache=/opt/templar/npm-cache \
    npm_config_audit=false \
    npm_config_fund=false \
    npm_config_update_notifier=false

# The toolchain, components and wasm32 target that rust-toolchain.toml pins.
RUN rustup toolchain install && rustc --version && cargo --version

# Every crate in Cargo.lock, for every target.
RUN cargo fetch --locked

# The redstone-bridge npm packages, from its package-lock.json. This also fills the npm cache, so
# the `npm install` that build.rs runs works offline if the scan rebuilds the bridge.
RUN cd service/redstone-bridge/js && npm ci --no-progress

# The neard sandbox binary at the version the locked near-sandbox crate defaults to. near-sandbox
# uses NEAR_SANDBOX_BIN_PATH and does not download when that file exists.
RUN set -eux; \
    case "$(uname -m)" in x86_64) platform=Linux-x86_64 ;; aarch64) platform=Linux-aarch64 ;; esac; \
    crate="$(awk '/^name = "near-sandbox"$/ { getline; gsub(/"/, "", $3); print $3; exit }' Cargo.lock)"; \
    neard="$(sed -n 's/^pub const DEFAULT_NEAR_SANDBOX_VERSION: &str = "\(.*\)";$/\1/p' \
      "${CARGO_HOME}"/registry/src/*/near-sandbox-"${crate}"/src/lib.rs | head -n1)"; \
    test -n "${neard}"; \
    tmp="$(mktemp -d)"; \
    curl -fsSL "https://s3-us-west-1.amazonaws.com/build.nearprotocol.com/nearcore/${platform}/${neard}/near-sandbox.tar.gz" \
      | tar -xzf - -C "$tmp"; \
    mkdir -p "$(dirname "${NEAR_SANDBOX_BIN_PATH}")"; \
    install -m 0755 "$(find "$tmp" -type f -name near-sandbox | head -n1)" "${NEAR_SANDBOX_BIN_PATH}"; \
    rm -rf "$tmp"; \
    echo "neard sandbox ${neard} for near-sandbox ${crate}"

# The released contract Wasms the migration and upgrade tests deploy, checked against the
# SHA-256 values in contract/artifacts/releases/.
RUN just artifacts-fetch

# Every test binary, then every workspace binary (CI's "Binary Build" job).
RUN cargo nextest run --locked --workspace --no-run \
    && cargo build --locked --workspace --bins

# The NEAR contract Wasms the node-backed tests deploy, in target/near.
RUN bash script/prebuild-test-contracts.sh

# Postgres for the #[sqlx::test] fixtures: the role and database CI's service container creates.
# `templar-postgres-up` starts it in the offline image. The agent runs as root, and pg_ctlcluster
# switches to the postgres user.
RUN set -eux; \
    printf '%s\n' '#!/bin/sh' 'set -e' \
      'version="$(ls /etc/postgresql | sort -V | tail -n1)"' \
      'pg_ctlcluster "$version" main status >/dev/null 2>&1 || pg_ctlcluster "$version" main start' \
      'until pg_isready -q -h localhost -p 5432; do sleep 1; done' \
      > /usr/local/bin/templar-postgres-up; \
    printf '%s\n' '#!/bin/sh' \
      'pg_ctlcluster "$(ls /etc/postgresql | sort -V | tail -n1)" main stop' \
      > /usr/local/bin/templar-postgres-down; \
    chmod 0755 /usr/local/bin/templar-postgres-up /usr/local/bin/templar-postgres-down; \
    templar-postgres-up; \
    su postgres -c "psql -v ON_ERROR_STOP=1 \
      -c \"CREATE ROLE relayeruser LOGIN SUPERUSER PASSWORD 'password'\" \
      -c \"CREATE DATABASE relayer OWNER relayeruser\""; \
    templar-postgres-down

# Run the fast gate as CI does, so the build log shows the image works. A failing or slow test
# does not fail the build. The cap keeps the whole build inside the scanner's 45-minute limit.
# The node-backed gate (just test-sandbox) is left for the scan.
RUN templar-postgres-up \
    && (timeout 600 env NEXTEST_PROFILE=ci just test-fast \
        || echo "WARNING: just test-fast reported failures or ran out of time") \
    ; templar-postgres-down

# Everything is fetched. From here on, cargo, npm and the artifact cache must not use the network.
ENV CARGO_NET_OFFLINE=true \
    TEMPLAR_ARTIFACT_OFFLINE=1 \
    npm_config_offline=true

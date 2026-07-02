#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/prebuild-test-contracts.sh"

# `#[sqlx::test]` reads DATABASE_URL at runtime to provision per-test databases.
# Respect a caller-provided value; only when it is unset do we boot the local
# compose Postgres (waiting until it is healthy) and point at it. This avoids
# starting a container on machines without Docker or with 5432 already in use.
if [ -z "${DATABASE_URL:-}" ]; then
    docker compose \
        --file "${ROOT_DIR}/service/relayer/compose.dev.yaml" up postgres \
        --detach --wait
    export DATABASE_URL="postgres://relayeruser:password@localhost:5432/relayer"
fi

# Run tests with nextest profile (defaults to 'ci' in CI via NEXTEST_PROFILE env var)
cargo nextest run "$@"

# Build-artifact cleanup is intentionally NOT done here. Disk is managed by
# reducing debug info (CARGO_PROFILE_*_DEBUG=line-tables-only in CI) and by
# Swatinem/rust-cache's own cache-aware pruning. The previous
# `find … -name '*.rmeta' -delete` ran *after* the tests (too late to relieve
# in-run disk pressure) and stripped dependency metadata from the saved cache,
# forcing those deps to recompile next run — spending time to save disk that did
# not actually materialize.

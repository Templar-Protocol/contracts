#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
"$SCRIPT_DIR/prebuild-test-contracts.sh"
export TEST_CONTRACTS_PREBUILT=1

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

# Clean up build artifacts to save disk space in CI
if [ -n "$CI" ]; then
    echo "Cleaning up build artifacts to save disk space..."
    # Remove only the largest intermediate artifacts
    find target -type f -name "*.rmeta" -delete 2>/dev/null || true
    # Clean up incremental compilation artifacts
    rm -rf target/debug/incremental 2>/dev/null || true
    rm -rf target/release 2>/dev/null || true
    # Show remaining disk space
    df -h
fi

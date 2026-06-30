#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/prebuild-test-contracts.sh"

# start database for relayer / gateway-store tests, waiting until it is healthy
# (the `#[sqlx::test]` tests connect immediately to create per-test databases)
docker compose \
    --file "${ROOT_DIR}/service/relayer/compose.dev.yaml" up postgres \
    --detach --wait

# `#[sqlx::test]` reads DATABASE_URL at runtime to provision per-test databases.
# Respect an existing value; otherwise point at the compose postgres above.
export DATABASE_URL="${DATABASE_URL:-postgres://relayeruser:password@localhost:5432/relayer}"

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

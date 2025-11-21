#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/./prebuild-test-contracts.sh"

# start database for relayer tests
docker compose \
    --file "${ROOT_DIR}/service/relayer/compose.dev.yaml" up postgres \
    --detach

# Run tests with nextest profile (defaults to 'ci' in CI via NEXTEST_PROFILE env var)
cargo nextest run "$@"

# Clean up build artifacts to save disk space in CI
if [ -n "$CI" ]; then
    echo "Cleaning up build artifacts to save disk space..."
    # Remove debug artifacts and keep only essentials
    find target -type f -name "*.rmeta" -delete 2>/dev/null || true
    find target -type f -name "*.rlib" -delete 2>/dev/null || true
    cargo clean --release 2>/dev/null || true
    # Show remaining disk space
    df -h
fi

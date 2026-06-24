#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/prebuild-test-contracts.sh"

# start database for relayer tests
docker compose \
    --file "${ROOT_DIR}/service/relayer/compose.dev.yaml" up postgres \
    --detach

# Run tests with nextest profile (defaults to 'ci' in CI via NEXTEST_PROFILE env var)
cargo nextest run "$@"

# Build-artifact cleanup is intentionally NOT done here. Disk is managed by
# reducing debug info (CARGO_PROFILE_*_DEBUG=line-tables-only in CI) and by
# Swatinem/rust-cache's own cache-aware pruning. The previous
# `find … -name '*.rmeta' -delete` ran *after* the tests (too late to relieve
# in-run disk pressure) and stripped dependency metadata from the saved cache,
# forcing those deps to recompile next run — spending time to save disk that did
# not actually materialize.

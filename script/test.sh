#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
"$SCRIPT_DIR/prebuild-test-contracts.sh"
export TEST_CONTRACTS_PREBUILT=1

# `#[sqlx::test]` reads DATABASE_URL at runtime to provision per-test databases;
# provision (or reuse) the compose Postgres and export DATABASE_URL.
source "$SCRIPT_DIR/postgres-up.sh"

# Run tests with nextest profile (defaults to 'ci' in CI via NEXTEST_PROFILE env
# var). This is the fast/default gate; the node-backed suite runs separately via
# the `sandbox` profile (see `just test` / `just test-sandbox` and CI).
cargo nextest run "$@"

# Build-artifact cleanup is intentionally NOT done here. Disk is managed by
# reducing debug info (CARGO_PROFILE_*_DEBUG=line-tables-only in CI) and by
# Swatinem/rust-cache's own cache-aware pruning. The previous
# `find … -name '*.rmeta' -delete` ran *after* the tests (too late to relieve
# in-run disk pressure) and stripped dependency metadata from the saved cache,
# forcing those deps to recompile next run — spending time to save disk that did
# not actually materialize.

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

# No build-artifact cleanup here: deleting *.rmeta after the run strips dependency
# metadata from the saved cache and forces those deps to recompile next run. Disk
# is managed via line-tables-only debug info (CI) + rust-cache's own pruning.

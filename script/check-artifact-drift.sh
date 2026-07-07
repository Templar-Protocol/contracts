#!/usr/bin/env bash
# Verify the checked-in embedded WASM catalog is self-consistent:
#   - every blob in res/near/ hashes to the `expected_sha256` pinned in ids.rs
#   - every catalog `version` matches its contract's Cargo.toml version
# These are pure, in-memory checks — no contract builds, no Docker, seconds.
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
cargo test -p templar-contract-artifacts --features embedded-wasm,workspace-loader \
    drift_check -- --include-ignored --nocapture

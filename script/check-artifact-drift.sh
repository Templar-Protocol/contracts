#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
"$SCRIPT_DIR/prebuild-test-contracts.sh" --profile drift
cargo test -p templar-contract-artifacts --features embedded-wasm,workspace-loader drift_check -- --ignored --nocapture

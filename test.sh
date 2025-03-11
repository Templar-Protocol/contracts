#!/usr/bin/env bash
set -e

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "$ROOT_DIR/mock/oracle"
cargo near build non-reproducible-wasm

cd "$ROOT_DIR/mock/ft"
cargo near build non-reproducible-wasm

cd "$ROOT_DIR/contract/market"
cargo near build non-reproducible-wasm

cd "$ROOT_DIR"
export TEST_CONTRACTS_PREBUILT=1
cargo nextest run --retries 1 "$@"

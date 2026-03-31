#!/usr/bin/env bash
set -ex

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

cd "$ROOT_DIR/mock/oracle"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/mock/ft"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/mock/mt"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/registry"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/market"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/redstone-adapter"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/lst-oracle"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/proxy-oracle"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/universal-account"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR/contract/vault/near"
cargo near build non-reproducible-wasm 1>&2

cd "$ROOT_DIR"
export TEST_CONTRACTS_PREBUILT=1

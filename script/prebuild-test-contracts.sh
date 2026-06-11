#!/usr/bin/env bash
set -ex

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"
# cargo-near rejects Rust >=1.87 for this NEAR WASM build path, so the
# prebuild toolchain intentionally stays separate from the repo lint/test pin.
NEAR_RUST_TOOLCHAIN="${NEAR_RUST_TOOLCHAIN:-1.86.0}"

cargo_near_build() {
    RUSTUP_TOOLCHAIN="$NEAR_RUST_TOOLCHAIN" cargo near build non-reproducible-wasm 1>&2
}

cd "$ROOT_DIR/mock/oracle"
cargo_near_build

cd "$ROOT_DIR/mock/ft"
cargo_near_build

cd "$ROOT_DIR/mock/mt"
cargo_near_build

cd "$ROOT_DIR/contract/registry"
cargo_near_build

cd "$ROOT_DIR/contract/market"
cargo_near_build

cd "$ROOT_DIR/contract/redstone-adapter"
cargo_near_build

cd "$ROOT_DIR/contract/proxy-oracle/near/lst-contract"
cargo_near_build

cd "$ROOT_DIR/contract/proxy-oracle/near/contract"
cargo_near_build

cd "$ROOT_DIR/contract/proxy-oracle/near/governance-contract"
cargo_near_build

cd "$ROOT_DIR/contract/universal-account"
cargo_near_build

cd "$ROOT_DIR/contract/vault/near"
cargo_near_build

cd "$ROOT_DIR"
export TEST_CONTRACTS_PREBUILT=1

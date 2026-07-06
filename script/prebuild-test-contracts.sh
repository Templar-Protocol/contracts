#!/usr/bin/env bash
set -ex

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

cd "$ROOT_DIR"
cargo run -p templar-contract-artifacts --features workspace-loader,clap --bin prebuild-test-contracts -- --workspace-root "$ROOT_DIR" 1>&2
export TEST_CONTRACTS_PREBUILT=1

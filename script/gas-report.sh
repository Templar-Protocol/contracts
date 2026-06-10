#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/../prebuild-test-contracts.sh"

cargo run --package templar-market-contract --example gas_report
cargo run --package templar-vault-contract --example gas_report

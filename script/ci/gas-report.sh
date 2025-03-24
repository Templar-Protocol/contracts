#!/usr/bin/env bash
set -e

source ../prebuild-test-contracts.sh

cargo run --package templar-market-contract --example gas_report

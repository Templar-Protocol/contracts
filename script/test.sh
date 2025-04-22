#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/./prebuild-test-contracts.sh"

cargo nextest run "$@"

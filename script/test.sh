#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/./prebuild-test-contracts.sh"

# start database for relayer tests
docker compose \
    --file "${ROOT_DIR}/service/relayer/compose.dev.yaml" up postgres \
    --detach

cargo nextest run "$@"

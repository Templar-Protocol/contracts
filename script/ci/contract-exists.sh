#!/usr/bin/env bash

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--network:NETWORK" "$@"

if [ -z "$NETWORK" ]; then
  NETWORK="testnet"
fi

near contract download-wasm ${ACCOUNT_ID} save-to-file /tmp/${ACCOUNT_ID}.wasm network-config ${NETWORK} now > /dev/null 2>&1 || true
if [ -f "/tmp/${ACCOUNT_ID}.wasm" ]; then
  echo 1
fi

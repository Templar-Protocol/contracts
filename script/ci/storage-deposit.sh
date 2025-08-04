#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--contract:CONTRACT_ID,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

near account \
  manage-storage-deposit "${CONTRACT_ID}" \
  deposit "${ACCOUNT_ID}" '0.00125 NEAR' \
  sign-as "${ACCOUNT_ID}" \
  network-config "${NETWORK}" \
  sign-with-plaintext-private-key "${PRIVATE_KEY}" \
  send

echo "Done"

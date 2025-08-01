#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--token:TOKEN_ID,--beneficiary:BENEFICIARY_ID,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
  NETWORK="testnet"
fi

echo "Recovering $TOKEN_ID tokens for $ACCOUNT_ID on $NETWORK"

echo "Transferring balance to $BENEFICIARY_ID"

set +e # send all errors if balance is zero
near tokens "$ACCOUNT_ID" send-ft "$TOKEN_ID" "$BENEFICIARY_ID" all memo "" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key "$PRIVATE_KEY" \
  send
set -e

echo "Performing storage unregistration"

near contract call-function as-transaction "$TOKEN_ID" storage_unregister \
  json-args '{"force":true}' \
  prepaid-gas '100.0 Tgas' \
  attached-deposit '1 yoctoNEAR' \
  sign-as "$ACCOUNT_ID" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key "$PRIVATE_KEY" \
  send

echo "Done"

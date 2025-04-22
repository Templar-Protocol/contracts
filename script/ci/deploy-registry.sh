#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--init:INIT,--network:NETWORK,--private-key:PRIVATE_KEY,--public-key:PUBLIC_KEY" "$@"

if [ -z "$NETWORK" ]; then
  NETWORK="testnet"
fi

if [ -z "$INIT" ]; then
  INIT=true
fi

cd "${SCRIPT_DIR}/../../contract/registry"

if $INIT; then
    echo "Deploying registry contract to ${ACCOUNT_ID} on ${NETWORK} with initialization call"
    # cargo near deploy build-reproducible-wasm --skip-git-remote-check "${ACCOUNT_ID}" \
    cargo near deploy build-non-reproducible-wasm "${ACCOUNT_ID}" \
        with-init-call new \
            json-args '{}' \
            prepaid-gas '100.0 Tgas' \
            attached-deposit '0 NEAR' \
        network-config "${NETWORK}" \
        sign-with-plaintext-private-key \
            --signer-public-key "${PUBLIC_KEY}" \
            --signer-private-key "${PRIVATE_KEY}" \
        send
else
    echo "Deploying registry contract to ${ACCOUNT_ID} on ${NETWORK} without initialization call"
    # cargo near deploy build-reproducible-wasm --skip-git-remote-check "${ACCOUNT_ID}" \
    cargo near deploy build-non-reproducible-wasm "${ACCOUNT_ID}" \
        without-init-call \
        network-config "${NETWORK}" \
        sign-with-plaintext-private-key \
            --signer-public-key "${PUBLIC_KEY}" \
            --signer-private-key "${PRIVATE_KEY}" \
        send
fi

echo "Done"

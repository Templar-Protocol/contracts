#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--version-key:VERSION_KEY,--network:NETWORK,--private-key:PRIVATE_KEY,--public-key:PUBLIC_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

near contract call-function as-transaction "${REGISTRY_ID}" remove_version \
    json-args "{\"version_key\":\"${VERSION_KEY}\"}" \
    prepaid-gas '200.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as "${ACCOUNT_ID}" \
    network-config testnet \
    sign-with-plaintext-private-key \
        --signer-public-key "${PUBLIC_KEY}" \
        --signer-private-key "${PRIVATE_KEY}" \
    send

echo "Done"

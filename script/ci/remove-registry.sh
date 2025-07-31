#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--beneficiary:BENEFICIARY_ID,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

EXISTS=$($SCRIPT_DIR/account-exists.sh \
    --account "$ACCOUNT_ID" \
    --network "$NETWORK")

if [[ -z "$EXISTS" ]]; then
    echo "Account does not exist, nothing to do"
    exit 0
fi

$SCRIPT_DIR/remove-all-versions-from-registry.sh \
    --account       "${ACCOUNT_ID}" \
    --registry      "${ACCOUNT_ID}" \
    --network       "${NETWORK}" \
    --private-key   "${PRIVATE_KEY}"

near account delete-account "${ACCOUNT_ID}" \
    beneficiary "${BENEFICIARY_ID}" \
    network-config "${NETWORK}" \
    sign-with-plaintext-private-key "${PRIVATE_KEY}" \
    send

echo "Done"

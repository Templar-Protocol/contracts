#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--beneficiary:BENEFICIARY_ID,--network:NETWORK,--private-key:PRIVATE_KEY,--public-key:PUBLIC_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

CONFIG=$(near --quiet contract call-function as-read-only "${ACCOUNT_ID}" get_configuration \
    json-args {} \
    network-config "${NETWORK}" \
    now)

echo "Configuration"
echo "$CONFIG" | jq .

BORROW_ID=$(echo "${CONFIG}" | jq -r '.borrow_asset.Nep141')
if [ -n "$BORROW_ID" ]; then
    echo "Recovering ${BORROW_ID} NEP-141 tokens"

    # $SCRIPT_DIR/recover-nep141.sh \
    #     --account       "${ACCOUNT_ID}" \
    #     --token         "${BORROW_ID}" \
    #     --beneficiary   "${BENEFICIARY_ID}" \
    #     --network       "${NETWORK}" \
    #     --public-key    "${PUBLIC_KEY}" \
    #     --private-key   "${PRIVATE_KEY}"
fi

COLLATERAL_ID=$(echo "${CONFIG}" | jq -r '.collateral_asset.Nep141')
if [ -n "$COLLATERAL_ID" ]; then
    echo "Recovering ${COLLATERAL_ID} NEP-141 tokens"

    # $SCRIPT_DIR/recover-nep141.sh \
    #     --account       "${ACCOUNT_ID}" \
    #     --token         "${COLLATERAL_ID}" \
    #     --beneficiary   "${BENEFICIARY_ID}" \
    #     --network       "${NETWORK}" \
    #     --public-key    "${PUBLIC_KEY}" \
    #     --private-key   "${PRIVATE_KEY}"
fi

echo "Deleting account ${ACCOUNT_ID}"

# near account delete-account "${ACCOUNT_ID}" \
#   beneficiary "${BENEFICIARY_ID}" \
#   network-config "${NETWORK}" \
#   sign-with-plaintext-private-key \
#     --signer-public-key "${PUBLIC_KEY}" \
#     --signer-private-key "${PRIVATE_KEY}" \
#   send

echo "Done"

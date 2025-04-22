#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--network:NETWORK,--private-key:PRIVATE_KEY,--public-key:PUBLIC_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

near contract call-function as-read-only "${REGISTRY_ID}" list_deployments \
    json-args "{}" \
    network-config "${NETWORK}" \
    now 2>&1 | view_json | jq -r '.[]' | \
while read MARKET_ID; do
    echo "Removing ${MARKET_ID}..."

    CONFIG=$(near contract call-function as-read-only "${MARKET_ID}" get_configuration \
        json-args {} \
        network-config "${NETWORK}" \
        now 2>&1 | view_json)

    BORROW_ID=$(echo "${CONFIG}" | jq -r '.borrow_asset.Nep141')
    if [ -n "$BORROW_ID" ]; then
        $SCRIPT_DIR/recover-nep141.sh \
            --account       "${MARKET_ID}" \
            --token         "${BORROW_ID}" \
            --beneficiary   "${REGISTRY_ID}" \
            --network       "${NETWORK}" \
            --public-key    "${PUBLIC_KEY}" \
            --private-key   "${PRIVATE_KEY}"
    fi

    COLLATERAL_ID=$(echo "${CONFIG}" | jq -r '.collateral_asset.Nep141')
    if [ -n "$COLLATERAL_ID" ]; then
        $SCRIPT_DIR/recover-nep141.sh \
            --account       "${MARKET_ID}" \
            --token         "${COLLATERAL_ID}" \
            --beneficiary   "${REGISTRY_ID}" \
            --network       "${NETWORK}" \
            --public-key    "${PUBLIC_KEY}" \
            --private-key   "${PRIVATE_KEY}"
    fi

    near account delete-account "${MARKET_ID}" \
      beneficiary "${REGISTRY_ID}" \
      network-config "${NETWORK}" \
      sign-with-plaintext-private-key \
        --signer-public-key "${PUBLIC_KEY}" \
        --signer-private-key "${PRIVATE_KEY}" \
      send
done

echo "Done"

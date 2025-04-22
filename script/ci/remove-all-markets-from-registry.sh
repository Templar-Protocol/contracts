#!/usr/bin/env bash
set -e

while [[ $# -gt 0 ]]; do
    case "$1" in
        -a|--account)
            ACCOUNT_ID="$2"
            shift 2
            ;;
        -r|--registry)
            REGISTRY_ID="$2"
            shift 2
            ;;
        -n|--network)
            NETWORK="$2"
            shift 2
            ;;
        -s|--private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        -v|--public-key)
            PUBLIC_KEY="$2"
            shift 2
            ;;
        *)
            >&2 echo "Invalid option: $1"
            exit 1
            ;;
    esac
done

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")

source "$SCRIPT_DIR/utils.sh"

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

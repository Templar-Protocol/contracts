#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

DEPLOYMENTS=$(near --quiet contract call-function as-read-only "${REGISTRY_ID}" list_deployments \
    json-args "{}" \
    network-config "${NETWORK}" \
    now)

echo "Deployments"
echo "${DEPLOYMENTS}" | jq .

echo "${DEPLOYMENTS}" | jq -r '.[]' | while read MARKET_ID; do
    echo "Removing ${MARKET_ID}"

    "${SCRIPT_DIR}/remove-market.sh" \
        --account "${MARKET_ID}" \
        --beneficiary "${REGISTRY_ID}" \
        --network "${NETWORK}" \
        --private-key "${PRIVATE_KEY}"
done

echo "Done"

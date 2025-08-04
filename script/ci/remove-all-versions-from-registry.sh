#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

VERSIONS=$(near --quiet contract call-function as-read-only "${REGISTRY_ID}" list_versions \
    json-args "{}" \
    network-config "${NETWORK}" \
    now)

echo "Versions"
echo "$VERSIONS" | jq .

echo "${VERSIONS}" | jq -r '.[]' | while read VERSION_KEY; do
    echo "Removing ${VERSION_KEY}..."

    $SCRIPT_DIR/remove-version-from-registry.sh \
        --account       "${ACCOUNT_ID}" \
        --registry      "${ACCOUNT_ID}" \
        --version-key   "${VERSION_KEY}" \
        --network       "${NETWORK}" \
        --private-key   "${PRIVATE_KEY}"
done

echo "Done"

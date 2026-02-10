#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")

source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--version-key:VERSION_KEY,--deploy-mode:DEPLOY_MODE,--deposit:DEPOSIT,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$DEPLOY_MODE" ]; then
    DEPLOY_MODE="normal"
fi

if [ -z "$DEPOSIT" ]; then
    DEPOSIT="1 yoctoNEAR"
fi

cd "${SCRIPT_DIR}/../../contract/market"

echo "Building market"

cargo near build reproducible-wasm

"${SCRIPT_DIR}/add-version-to-registry.sh" \
    --package templar_market_contract \
    --account "${ACCOUNT_ID}" \
    --registry "${REGISTRY_ID}" \
    --version-key "${VERSION_KEY}" \
    --deploy-mode "${DEPLOY_MODE}" \
    --deposit "${DEPOSIT}" \
    --network "${NETWORK}" \
    --private-key "${PRIVATE_KEY}"

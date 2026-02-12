#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR=$(readlink -f "$SCRIPT_DIR/../..")

source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--version-key:VERSION_KEY,--deploy-mode:DEPLOY_MODE,--deposit:DEPOSIT,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$REGISTRY_ID" ]; then
    REGISTRY_ID="${ACCOUNT_ID}"
fi

if [ -z "$DEPLOY_MODE" ]; then
    DEPLOY_MODE="global_hash"
fi

if [ -z "$DEPOSIT" ]; then
    if [ "$DEPLOY_MODE" = "global_hash" ]; then
        local size;
        size=$(stat --printf="%s" "$ROOT_DIR/target/near/templar_universal_account_contract/templar_universal_account_contract.wasm")
        DEPOSIT=$(bc -l <<< "$size / 10000")
        local available;
        available=$(near --quiet tokens "$ACCOUNT_ID" view-near-balance network-config mainnet now | grep -oP '(?<=has )\d*(\.\d+)?(?= NEAR)')
        if (( $(bc -l <<< "$available < $DEPOSIT") )); then
            >&2 echo "Insufficient balance: $available NEAR available, $DEPOSIT NEAR required for deposit"
            exit 1
        fi
    else
        DEPOSIT="1 yoctoNEAR"
    fi
fi

cd "${SCRIPT_DIR}/../../contract/universal-account"

echo "Building universal account contract"

cargo near build reproducible-wasm

"${SCRIPT_DIR}/add-version-to-registry.sh" \
    --package templar_universal_account_contract \
    --account "${ACCOUNT_ID}" \
    --registry "${REGISTRY_ID}" \
    --version-key "${VERSION_KEY}" \
    --deploy-mode "${DEPLOY_MODE}" \
    --deposit "${DEPOSIT}" \
    --network "${NETWORK}" \
    --private-key "${PRIVATE_KEY}"

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
        -k|--version-key)
            VERSION_KEY="$2"
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
            echo "Invalid option: $1"
            exit 1
            ;;
    esac
done

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")

cd "${SCRIPT_DIR}/../../contract/market"

echo "Building market"

cargo near build non-reproducible-wasm
# cargo near build reproducible-wasm


echo "Generating Borsh arguments"

ARGS_FILE=$(mktemp "/tmp/args-XXXXXX")
trap "rm -f $ARGS_FILE" EXIT

cargo run --package test-utils --example registry_add_version_args -- "${VERSION_KEY}" > $ARGS_FILE


echo "Creating new version on registry"

near contract call-function as-transaction "${REGISTRY_ID}" \
    add_version \
        file-args "${ARGS_FILE}" \
        prepaid-gas '300.0 Tgas' \
        attached-deposit '1 yoctoNEAR' \
    sign-as "${ACCOUNT_ID}" \
    network-config "${NETWORK}" \
    sign-with-plaintext-private-key \
        --signer-public-key "${PUBLIC_KEY}" \
        --signer-private-key "${PRIVATE_KEY}" \
    send


echo "Done"

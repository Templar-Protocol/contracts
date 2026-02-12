#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")

source "$SCRIPT_DIR/utils.sh"

parse_args "--package:PACKAGE,--account:ACCOUNT_ID,--registry:REGISTRY_ID,--version-key:VERSION_KEY,--deploy-mode:DEPLOY_MODE,--deposit:DEPOSIT,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

if [ -z "$DEPOSIT" ]; then
    DEPOSIT="1 yoctoNEAR"
fi


echo "Generating Borsh arguments"

ARGS_FILE=$(mktemp "/tmp/args-XXXXXX")
trap "rm -f $ARGS_FILE" EXIT

cargo run --package test-utils --example registry_add_version_args -- "${PACKAGE}" "${VERSION_KEY}" "${DEPLOY_MODE}" > $ARGS_FILE


echo "Creating new version on registry"

near contract call-function as-transaction "${REGISTRY_ID}" \
    add_version \
        file-args "${ARGS_FILE}" \
        prepaid-gas '300.0 Tgas' \
        attached-deposit "${DEPOSIT}" \
    sign-as "${ACCOUNT_ID}" \
    network-config "${NETWORK}" \
    sign-with-plaintext-private-key "${PRIVATE_KEY}" \
    send

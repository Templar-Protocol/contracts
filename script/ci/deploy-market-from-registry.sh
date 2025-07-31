#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--registry:REGISTRY_ID,--version-key:VERSION_KEY,--init-args:INIT_ARGS,--name:NAME,--with-full-access-key:WITH_FULL_ACCESS_KEY,--network:NETWORK,--private-key:PRIVATE_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

if [ -z "$NAME" ]; then
    NAME="null"
else
    NAME='"'$NAME'"'
fi

if [ -z "$WITH_FULL_ACCESS_KEY" ]; then
    FULL_ACCESS_KEYS="null"
else
    FULL_ACCESS_KEYS=$(jq --null-input --arg key "${WITH_FULL_ACCESS_KEY}" '[$key]')
fi

ARGS=$(jq --null-input \
    --argjson name "${NAME}" \
    --arg version_key "${VERSION_KEY}" \
    --arg init_args "$(echo -n "${INIT_ARGS}" | base64 | tr -d \\n)" \
    --argjson full_access_keys "${FULL_ACCESS_KEYS}" \
    '$ARGS.named')

echo "Generated deployment args"
echo "${ARGS}" | jq .

near contract call-function as-transaction "${REGISTRY_ID}" \
    deploy_market \
        json-args "${ARGS}" \
        prepaid-gas '300.0 Tgas' \
        attached-deposit '5 NEAR' \
    sign-as "${ACCOUNT_ID}" \
    network-config "${NETWORK}" \
    sign-with-plaintext-private-key "${PRIVATE_KEY}" \
    send


echo "Done"

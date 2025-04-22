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
        --init-args)
            INIT_ARGS="$2"
            shift 2
            ;;
        --name)
            NAME="$2"
            shift 2
            ;;
        --with-full-access-key)
            WITH_FULL_ACCESS_KEY="$2"
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

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")

ARGS=$(jq --null-input \
    --argjson name "${NAME}" \
    --arg version_key "${VERSION_KEY}" \
    --arg init_args "$(echo $INIT_ARGS | base64)" \
    --argjson full_access_keys "${FULL_ACCESS_KEYS}" \
    '$ARGS.named')

>&2 echo "Generated deployment args"
>&2 echo "${ARGS}"

near contract call-function as-transaction "${REGISTRY_ID}" \
    deploy_market \
        json-args "${ARGS}" \
        prepaid-gas '300.0 Tgas' \
        attached-deposit '5 NEAR' \
    sign-as "${ACCOUNT_ID}" \
    network-config "${NETWORK}" \
    sign-with-plaintext-private-key \
      --signer-public-key "${PUBLIC_KEY}" \
      --signer-private-key "${PRIVATE_KEY}" \
    send


>&2 echo "Done"
